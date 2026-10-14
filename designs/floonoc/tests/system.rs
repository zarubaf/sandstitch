// SPDX-License-Identifier: Apache-2.0

//! The mesh generator's checks, the small system in Slang, and the
//! simulation harnesses in Verilator: one core next to one memory, the
//! small system, and a core whose writes go to a window without memory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use floonoc::*;

use topstitch::{EmitOptions, IO, ModDef};

#[path = "../../../tests/common/slang.rs"]
#[allow(dead_code)]
mod slang;
use slang::elaborate_manifest_vendored;

/// `third_party/<rel>`.
fn third_party(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("third_party")
        .join(rel)
}

/// Emits `top` and elaborates it in Slang from its manifest, tolerating
/// the implicit width conversions of the vendored RTL under
/// `third_party/<vendored>`; the emitted RTL and the fixtures stay strict.
/// Returns the emitted text.
fn elaborate(top: &ModDef, vendored: &str) -> String {
    let emitted = top.emit(EmitOptions::default());
    let manifest = top.manifest();
    if let Err(slang) =
        elaborate_manifest_vendored(&manifest, &emitted, &top.get_name(), &third_party(vendored))
    {
        panic!("Slang rejects the emitted design:\n{emitted}\n{slang}");
    }
    emitted
}

/// The flags of doc/design/reference-noc.md, decision 10.
const VERILATOR_FLAGS: [&str; 8] = [
    "--binary",
    "--timing",
    "--timescale",
    "1ns/1ps",
    "-Wno-fatal",
    "-Wno-PINNOTFOUND",
    "-j",
    "0",
];

/// The wall-clock limit of a simulation run; `tb_driver`'s cycle timeout
/// ends a healthy one long before.
const SIM_LIMIT: Duration = Duration::from_secs(120);

/// What a simulation printed and whether it finished.
struct Simulation {
    /// The simulation exited successfully (`$finish`, not `$stop` or an
    /// assertion).
    finished: bool,
    /// Standard output and standard error.
    output: String,
}

/// Whether Verilator is on `PATH`. Without it the test fails if `CI` is
/// set and otherwise skips with a message.
fn have_verilator(test: &str) -> bool {
    if Command::new("verilator").arg("--version").output().is_ok() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "{test}: verilator is not on PATH and CI is set"
    );
    eprintln!("{test}: skipped, verilator is not on PATH (nix develop provides it)");
    false
}

/// Builds `top` with Verilator from its emitted RTL and manifest (sources,
/// include and library directories, library files and extensions, macro
/// definitions) in a temporary directory, and runs it. Panics if the
/// build fails.
fn simulate(top: &ModDef) -> Simulation {
    let name = top.get_name();
    let emitted = top.emit(EmitOptions::default());
    let manifest = top.manifest();
    let dir = tempfile::tempdir().unwrap();
    let rtl = dir.path().join(format!("{name}.sv"));
    std::fs::write(&rtl, &emitted).unwrap();
    let obj = dir.path().join("obj");

    let mut cmd = Command::new("verilator");
    cmd.args(VERILATOR_FLAGS)
        .arg("--top-module")
        .arg(&name)
        .arg("--Mdir")
        .arg(&obj);
    for incdir in &manifest.incdirs {
        cmd.arg(format!("-I{}", incdir.display()));
    }
    for (name, value) in &manifest.defines {
        cmd.arg(format!("+define+{name}={value}"));
    }
    for libdir in &manifest.libdirs {
        cmd.arg("-y").arg(libdir);
    }
    for libfile in &manifest.libfiles {
        cmd.arg("-v").arg(libfile);
    }
    for ext in &manifest.libexts {
        cmd.arg(format!("+libext+{ext}"));
    }
    cmd.args(&manifest.sources).arg(&rtl);

    let start = Instant::now();
    let out = cmd.output().expect("running verilator");
    let build = start.elapsed();
    assert!(
        out.status.success(),
        "verilator fails on {name}:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let log = dir.path().join("sim.log");
    let file = std::fs::File::create(&log).unwrap();
    let start = Instant::now();
    let mut child = Command::new(obj.join(format!("V{name}")))
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .expect("running the simulation");
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > SIM_LIMIT {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "the simulation of {name} ran longer than {SIM_LIMIT:?}:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let run = start.elapsed();
    let output = std::fs::read_to_string(&log).unwrap();
    println!("{name}: verilator build {build:.1?}, run {run:.1?}\n{output}");
    Simulation {
        finished: status.success(),
        output,
    }
}

/// The simulation finished and the driver printed `PASS`.
fn passed(sim: &Simulation) -> bool {
    sim.finished && sim.output.lines().any(|l| l.starts_with("PASS"))
}

/// No `PASS`: the simulation finished with the driver's `FAIL`, or stopped.
fn failed(sim: &Simulation) -> bool {
    !sim.output.lines().any(|l| l.starts_with("PASS"))
        && (!sim.finished || sim.output.lines().any(|l| l.starts_with("FAIL")))
}

/// A harness of one single-core compute tile at (0, 0) whose link
/// connects straight to a memory tile at (1, 1), no router between them,
/// with the driver and a memory check that believes it sits at `checked`
/// and expects one write burst. The packages go to `dir`.
fn back_to_back(checked: (usize, usize), dir: &Path) -> ModDef {
    let cfg = FlooConfig::synth("b2b");
    let target = Target {
        x: 1,
        y: 1,
        seed: 7,
    };
    let mut spec = MeshSpec::new(2, 2);
    spec.set(1, 1, Endpoint::Memory).set(
        0,
        0,
        Endpoint::Compute {
            targets: vec![target],
        },
    );
    let sys = sys_pkg(&cfg, &spec);
    let compute_def = compute_tile(&cfg, &sys, "b2b_compute", &[target], dir);
    let memory_def = memory_tile(&cfg, "b2b_memory", dir);

    let top = ModDef::new("b2b_harness");
    let compute = top.instantiate(&compute_def, Some("i_compute"), None);
    let memory = top.instantiate(&memory_def, Some("i_memory"), None);
    let driver = top.instantiate(&tb_driver(1, 1), Some("i_driver"), None);
    let mem = top.instantiate(&mem_check(&cfg, checked, 1, dir), Some("i_mem"), None);
    let (clk, rst) = (driver.get_port("clk_o"), driver.get_port("rst_no"));
    for inst in [&compute, &memory, &mem] {
        clk.connect(&inst.get_port("clk_i"));
        rst.connect(&inst.get_port("rst_ni"));
    }
    tie_node_id(&compute, 0, 0);
    tie_node_id(&memory, 1, 1);
    compute
        .get_intf(LINK)
        .connect(&memory.get_intf(LINK), false);
    for port in ["done", "error"] {
        compute
            .get_port(format!("{port}_o"))
            .connect(&driver.get_port(format!("{port}_i")));
    }
    memory
        .get_port("axi_req_o")
        .connect(&mem.get_port("axi_req_i"));
    memory
        .get_port("axi_rsp_i")
        .connect(&mem.get_port("axi_rsp_o"));
    mem.get_port("ok_o").connect(&driver.get_port("ok_i"));
    mem.get_port("count_o").unused();
    top
}

/// The tiles and the fixtures work before any mesh does.
#[test]
fn core_reaches_memory() {
    if !have_verilator("core_reaches_memory") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let sim = simulate(&back_to_back((1, 1), dir.path()));
    assert!(passed(&sim), "{}", sim.output);
}

/// The same traffic, checked as if the memory sat at (0, 1): every beat
/// carries Y = 1 but X = 1, so the memory check reports each written beat
/// outside its window and the driver fails on its `ok`.
#[test]
fn memory_check_rejects_writes_outside_its_window() {
    if !have_verilator("memory_check_rejects_writes_outside_its_window") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let sim = simulate(&back_to_back((0, 1), dir.path()));
    assert!(sim.finished, "{}", sim.output);
    assert!(
        sim.output
            .contains("mem_check (0, 1): write to 0x1101c0 outside the window"),
        "{}",
        sim.output
    );
    assert!(
        sim.output.contains("FAIL: memory check ok 0"),
        "{}",
        sim.output
    );
    assert!(failed(&sim), "{}", sim.output);
}

#[test]
fn small_system_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = small();
    let system = system(&cfg, &spec, dir.path());
    let emitted = elaborate(&system, "floonoc/hw");

    // Three cores, in router order, and the memory's AXI port.
    assert_eq!(system.get_port("done_o").io(), IO::Output(3));
    assert_eq!(system.get_port("error_o").io(), IO::Output(3));
    for port in ["x1_y1_axi_req_o", "x1_y1_axi_rsp_i", "clk_i", "rst_ni"] {
        system.get_port(port);
    }
    assert_eq!(system.get_ports(None).len(), 6);

    // Each compute tile targets the memory's window through the system
    // package, which also holds every router's window and the memory base.
    assert_eq!(
        emitted
            .matches(".TargetBase(small_sys_pkg::Window_x1_y1)")
            .count(),
        3,
        "{emitted}"
    );
    let manifest = system.manifest();
    let sys_file = sys_pkg(&cfg, &spec).write(dir.path()).unwrap();
    assert!(manifest.sources.contains(&cfg_pkg_file(&cfg, dir.path())));
    assert!(manifest.sources.contains(&sys_file), "{manifest:#?}");
    let text = std::fs::read_to_string(&sys_file).unwrap();
    for decl in [
        "Window_x0_y0 = 0;",
        "Window_x0_y1 = 1048576;",
        "Window_x1_y0 = 65536;",
        "Window_x1_y1 = 1114112;",
        "MemBase_x1_y1 = 1114112;",
    ] {
        assert!(text.contains(decl), "{decl} in\n{text}");
    }

    // Four routers of one specialization, a tile at each local port.
    let routers: Vec<&(String, String)> = manifest
        .specializations
        .iter()
        .filter(|(path, _)| path.contains(".i_router_"))
        .collect();
    assert_eq!(routers.len(), 4, "{manifest:#?}");
    assert!(routers.iter().all(|(_, hash)| *hash == routers[0].1));
    for inst in [
        "i_compute_x0_y0",
        "i_compute_x0_y1",
        "i_compute_x1_y0",
        "i_memory_x1_y1",
    ] {
        assert!(emitted.contains(&format!(" {inst} (")), "{inst}");
    }
}

#[test]
fn small_harness_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = small();
    let harness = harness(&cfg, &spec, dir.path());
    // The harness adds `axi_sim_mem`, whose own width conversions are
    // tolerated like FlooNoC's.
    let emitted = elaborate(&harness, "");
    for param in [
        ".NumCores(32'h0000_0003)",
        ".NumMems(32'h0000_0001)",
        ".ExpectedWrites(32'h0000_0003)",
        ".X(32'h0000_0001)",
        ".AddrOffsetY(32'h0000_0014)",
        ".axi_req_t(small_pkg::axi_narrow_out_req_t)",
    ] {
        assert!(emitted.contains(param), "{param} in\n{emitted}");
    }
    assert!(harness.get_ports(None).is_empty());
}

#[test]
fn small_system_passes_traffic() {
    if !have_verilator("small_system_passes_traffic") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = small();
    let sim = simulate(&harness(&cfg, &spec, dir.path()));
    assert!(passed(&sim), "{}", sim.output);
}

/// The small system with the core at (0, 0) retargeted to the window of
/// (`x`, `y`), which has no memory, through the test-only override.
fn misrouted(x: usize, y: usize) -> (FlooConfig, MeshSpec) {
    let (cfg, mut spec) = small();
    spec.allow_unbacked_targets().set(
        0,
        0,
        Endpoint::Compute {
            targets: vec![Target { x, y, seed: 1 }],
        },
    );
    (cfg, spec)
}

/// The core at (0, 0) writes to the window of (0, 1), a compute tile: its
/// network interface has no subordinate port, and its
/// `NoNarrowSbrPortAwRequest` assertion stops the simulation (Verilator
/// 5.052 checks assertions without `--assert`). No `PASS`.
#[test]
fn misrouted_write_fails() {
    if !have_verilator("misrouted_write_fails") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = misrouted(0, 1);
    let sim = simulate(&harness(&cfg, &spec, dir.path()));
    assert!(failed(&sim), "{}", sim.output);
    assert!(
        sim.output
            .contains("i_compute_x0_y1.i_chimney.NoNarrowSbrPortAwRequest"),
        "{}",
        sim.output
    );
}

/// The core at (0, 0) writes to the window of (0, 1), whose local link is
/// tied off: the write never leaves the router, and the driver reports the
/// timeout as `FAIL`.
#[test]
fn unreachable_write_times_out() {
    if !have_verilator("unreachable_write_times_out") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (cfg, mut spec) = misrouted(0, 1);
    spec.set(0, 1, Endpoint::Tie);
    let sim = simulate(&harness(&cfg, &spec, dir.path()));
    assert!(sim.finished, "{}", sim.output);
    let timeout = format!("FAIL: timeout after {TIMEOUT_CYCLES} cycles, done 10");
    assert!(sim.output.contains(&timeout), "{}", sim.output);
    assert!(failed(&sim), "{}", sim.output);
}

#[test]
#[should_panic(expected = "compute tile (0, 0) targets (0, 1), which has no memory")]
fn mesh_rejects_target_without_memory() {
    let (cfg, mut spec) = small();
    let target = Target {
        x: 0,
        y: 1,
        seed: 1,
    };
    spec.set(
        0,
        0,
        Endpoint::Compute {
            targets: vec![target],
        },
    );
    mesh(&cfg, &spec, tempfile::tempdir().unwrap().path());
}

#[test]
fn mesh_spec_checks() {
    let check = |cfg: &FlooConfig, spec: &MeshSpec, msg: &str| {
        let err = spec.check(cfg).unwrap_err();
        assert!(err.contains(msg), "{err}");
    };
    let (cfg, spec) = small();
    assert_eq!(spec.check(&cfg), Ok(()));

    // 2-bit coordinates hold four routers per dimension.
    let wide = MeshSpec::new(5, 1);
    check(&cfg, &wide, "5x1 mesh exceeds the 2x2-bit coordinates");
    assert_eq!(MeshSpec::new(4, 4).check(&cfg), Ok(()));

    // Tiles need exactly one local port; exported links do not.
    let mut six = cfg.clone();
    six.num_routes = 6;
    check(&six, &spec, "num_routes 6 with compute or memory endpoints");
    let mut exported = MeshSpec::new(2, 2);
    exported.set(0, 0, Endpoint::Export);
    assert_eq!(exported.check(&six), Ok(()));

    let target = |x, y, seed| Target { x, y, seed };
    let with = |targets: Vec<Target>| {
        let mut spec = spec.clone();
        spec.set(0, 0, Endpoint::Compute { targets });
        spec
    };
    check(&cfg, &with(vec![]), "compute tile (0, 0) has no target");
    check(
        &cfg,
        &with(vec![target(2, 0, 1)]),
        "targets (2, 0) outside the 2x2 mesh",
    );
    check(
        &cfg,
        &with(vec![target(1, 1, 1024)]),
        "seed 1024 is not below 1024",
    );
    check(
        &cfg,
        &with(vec![target(1, 1, 1); 9]),
        "9 cores leave no ID bit",
    );
    let mut unbacked = with(vec![target(1, 0, 1)]);
    check(&cfg, &unbacked, "targets (1, 0), which has no memory");
    unbacked.allow_unbacked_targets();
    assert_eq!(unbacked.check(&cfg), Ok(()));
}

/// A non-square mesh with tied-off, exported and tile endpoints: the
/// generalized links and edges elaborate, and each local port goes where
/// its endpoint says.
#[test]
fn three_by_two_mesh_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = FlooConfig::synth("m32");
    let mut spec = MeshSpec::new(3, 2);
    spec.set(2, 1, Endpoint::Memory)
        .set(0, 1, Endpoint::Export)
        .set(
            0,
            0,
            Endpoint::Compute {
                targets: vec![Target {
                    x: 2,
                    y: 1,
                    seed: 9,
                }],
            },
        );
    let mesh = mesh(&cfg, &spec, dir.path());
    assert_eq!(mesh.get_name(), mesh_name(&cfg, &spec));
    assert_eq!(mesh.get_name(), "m32_mesh_3x2");
    let emitted = elaborate(&mesh, "floonoc/hw");
    let ports: Vec<String> = mesh
        .get_ports(None)
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    for port in [
        "x0_y0_done_o",
        "x0_y0_error_o",
        "x2_y1_axi_req_o",
        "x2_y1_axi_rsp_i",
        "x0_y1_p4_floo_req_i",
        "x0_y1_p4_floo_wide_o",
    ] {
        assert!(ports.iter().any(|p| p == port), "{port} in {ports:?}");
    }
    // Clock, reset, test enable, the compute status, the memory port and
    // the six exported link ports; the tied routers export nothing.
    assert_eq!(ports.len(), 3 + 2 + 2 + 6, "{ports:?}");
    assert_eq!(emitted.matches(") i_router_x").count(), 6);
}

#[test]
fn large_system_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = large();
    let system = system(&cfg, &spec, dir.path());
    let emitted = elaborate(&system, "floonoc/hw");

    // Fourteen cores, and both memories' AXI ports.
    assert_eq!(system.get_port("done_o").io(), IO::Output(14));
    assert_eq!(system.get_port("error_o").io(), IO::Output(14));
    for port in [
        "x2_y0_axi_req_o",
        "x2_y0_axi_rsp_i",
        "x0_y2_axi_req_o",
        "x0_y2_axi_rsp_i",
    ] {
        system.get_port(port);
    }
    assert_eq!(system.get_ports(None).len(), 8);
    for target in ["Window_x2_y0", "Window_x0_y2"] {
        assert_eq!(
            emitted
                .matches(&format!(".TargetBase(large_sys_pkg::{target})"))
                .count(),
            7,
            "{target} in\n{emitted}"
        );
    }
    let manifest = system.manifest();
    let routers = manifest
        .specializations
        .iter()
        .filter(|(path, _)| path.contains(".i_router_"))
        .count();
    assert_eq!(routers, 9, "{manifest:#?}");
    for (x, y) in spec.routers() {
        let kind = if spec.memories().contains(&(x, y)) {
            "memory"
        } else {
            "compute"
        };
        let inst = format!(" i_{kind}_x{x}_y{y} (");
        assert_eq!(emitted.matches(&inst).count(), 1, "{inst}");
    }
}

#[test]
fn large_system_passes_traffic() {
    if !have_verilator("large_system_passes_traffic") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = large();
    let sim = simulate(&harness(&cfg, &spec, dir.path()));
    assert!(passed(&sim), "{}", sim.output);
}

/// Both configurations send traffic across routers, the large one to two
/// memories from every tile, and their core counts differ.
#[test]
fn configurations_exercise_routing() {
    let configs = [small(), large()];
    for (cfg, spec) in &configs {
        assert_eq!(spec.check(cfg), Ok(()));
        let far = spec
            .cores()
            .iter()
            .map(|&(x, y, t)| x.abs_diff(t.x) + y.abs_diff(t.y))
            .max()
            .unwrap();
        assert!(far >= 2, "{}: farthest core at distance {far}", cfg.name);
        // Every core has its own seed, so its traffic is distinct.
        let mut seeds: Vec<u32> = spec.cores().iter().map(|&(_, _, t)| t.seed).collect();
        let cores = seeds.len();
        seeds.sort();
        seeds.dedup();
        assert_eq!(seeds.len(), cores, "{}: seeds repeat", cfg.name);
        // Every route crosses at least two routers: no core sits at its
        // memory's router.
        assert!(
            spec.cores().iter().all(|&(x, y, t)| (x, y) != (t.x, t.y)),
            "{}",
            cfg.name
        );
    }
    let large_spec = &configs[1].1;
    for (x, y) in large_spec.routers() {
        if let Endpoint::Compute { targets } = large_spec.endpoint(x, y) {
            let mut memories: Vec<_> = targets.iter().map(|t| (t.x, t.y)).collect();
            memories.sort();
            memories.dedup();
            assert_eq!(
                memories.len(),
                targets.len(),
                "tile ({x}, {y}) targets one memory twice"
            );
        }
    }
    assert_eq!(large_spec.memories().len(), 2);
    assert_eq!(large_spec.cores().len(), 14);
    assert_ne!(configs[0].1.core_counts(), large_spec.core_counts());
}
