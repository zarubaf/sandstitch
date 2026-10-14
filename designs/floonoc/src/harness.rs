// SPDX-License-Identifier: Apache-2.0

//! The simulation harness: the system with a checked memory per memory
//! endpoint and a driver that reports `PASS` or `FAIL`. Simulation only.

use std::path::Path;

use topstitch::{ModDef, ParameterValue};

use crate::{Endpoint, FlooConfig, LIBDIRS, MeshSpec, cfg_pkg, import, rtl, system, write_pkg};

/// The cycles `tb_driver` waits for every core after reset before it
/// reports a timeout. The small system finishes in 42; the large one in 87.
pub const TIMEOUT_CYCLES: u32 = 10_000;

/// The name of the harness module of `cfg`: `<name>_harness`.
pub fn harness_name(cfg: &FlooConfig) -> String {
    format!("{}_harness", cfg.name)
}

/// The simulation fixture `rtl/tb_driver.sv` for `cores` cores and `mems`
/// memories, timing out after [`TIMEOUT_CYCLES`]: it drives `clk_o` and
/// `rst_no`, takes `done_i[cores]`, `error_i[cores]` and `ok_i[mems]`, and
/// prints `PASS` or `FAIL`.
///
/// Panics if the import fails.
#[track_caller]
pub fn tb_driver(cores: usize, mems: usize) -> ModDef {
    let overrides = [
        ("NumCores", (cores as u32).into()),
        ("NumMems", (mems as u32).into()),
        ("TimeoutCycles", TIMEOUT_CYCLES.into()),
    ];
    import(
        "tb_driver",
        &[],
        &[rtl().join("tb_driver.sv")],
        &[],
        &overrides,
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

/// The simulation fixture `rtl/mem_check.sv` for the memory at (`x`, `y`)
/// of `cfg`, expecting `writes` write bursts, with the narrow subordinate
/// AXI types and widths of the configuration package (written to
/// `out_dir`): it serves `axi_req_i`/`axi_rsp_o` from `axi_sim_mem` and
/// raises `ok_o` while every written beat stayed in the window of (`x`,
/// `y`) and the burst count (`count_o`) equals `writes`.
///
/// Panics if `cfg` is invalid or the import fails.
#[track_caller]
pub fn mem_check(
    cfg: &FlooConfig,
    (x, y): (usize, usize),
    writes: usize,
    out_dir: &Path,
) -> ModDef {
    let pkg = cfg_pkg(cfg);
    let overrides: [(&str, ParameterValue); 13] = [
        ("axi_req_t", pkg.ty("axi_narrow_out_req_t")),
        ("axi_rsp_t", pkg.ty("axi_narrow_out_rsp_t")),
        ("AddrWidth", cfg.narrow.addr_width.into()),
        ("DataWidth", cfg.narrow.data_width.into()),
        ("IdWidth", cfg.narrow.out_id_width.into()),
        ("UserWidth", cfg.narrow.user_width.into()),
        ("X", (x as u32).into()),
        ("Y", (y as u32).into()),
        ("XBits", cfg.x_bits.into()),
        ("YBits", cfg.y_bits.into()),
        ("AddrOffsetX", cfg.addr_offset_x.into()),
        ("AddrOffsetY", cfg.addr_offset_y.into()),
        ("ExpectedWrites", (writes as u32).into()),
    ];
    import(
        "mem_check",
        &[write_pkg(&pkg, out_dir)],
        &[rtl().join("mem_check.sv")],
        LIBDIRS,
        &overrides,
    )
    .unwrap_or_else(|e| panic!("{e}"))
}

/// The Verilator top `<name>_harness` of `cfg` and `spec`, without ports:
/// the [`system`] as `i_system`, one `mem_check` (`rtl/mem_check.sv`)
/// `i_mem_x<X>_y<Y>` per memory, serving the memory's AXI port and
/// expecting one write burst per core that targets it, and one
/// `tb_driver` (`rtl/tb_driver.sv`) `i_driver`, which drives clock and
/// reset, collects every core's `done` and `error` and every memory's
/// `ok`, waits at most [`TIMEOUT_CYCLES`] and prints `PASS` or `FAIL`.
/// Every configuration-dependent value reaches the fixtures as a parameter.
///
/// The packages are written to `out_dir`.
///
/// Panics if `spec` has no memory or an exported local link (which the
/// harness would leave open), or as [`system`] does.
#[track_caller]
pub fn harness(cfg: &FlooConfig, spec: &MeshSpec, out_dir: &Path) -> ModDef {
    let memories = spec.memories();
    assert!(
        !memories.is_empty(),
        "{}: a harness needs at least one memory",
        cfg.name
    );
    if let Some((x, y)) = spec
        .routers()
        .find(|&(x, y)| *spec.endpoint(x, y) == Endpoint::Export)
    {
        panic!(
            "{}: router ({x}, {y}) exports its local link, which a harness leaves open",
            cfg.name
        );
    }
    let cores = spec.cores();
    let system = system(cfg, spec, out_dir);
    let top = ModDef::new(harness_name(cfg));
    let sys = top.instantiate(&system, Some("i_system"), None);
    let driver_def = tb_driver(cores.len(), memories.len());
    let driver = top.instantiate(&driver_def, Some("i_driver"), None);
    let clk = driver.get_port("clk_o");
    let rst = driver.get_port("rst_no");
    clk.connect(&sys.get_port("clk_i"));
    rst.connect(&sys.get_port("rst_ni"));
    driver.get_port("done_i").connect(&sys.get_port("done_o"));
    driver.get_port("error_i").connect(&sys.get_port("error_o"));
    for (k, &(x, y)) in memories.iter().enumerate() {
        let writes = cores
            .iter()
            .filter(|(_, _, t)| (t.x, t.y) == (x, y))
            .count();
        let def = mem_check(cfg, (x, y), writes, out_dir);
        let mem = top.instantiate(&def, Some(&format!("i_mem_x{x}_y{y}")), None);
        clk.connect(&mem.get_port("clk_i"));
        rst.connect(&mem.get_port("rst_ni"));
        mem.get_port("axi_req_i")
            .connect(&sys.get_port(format!("x{x}_y{y}_axi_req_o")));
        mem.get_port("axi_rsp_o")
            .connect(&sys.get_port(format!("x{x}_y{y}_axi_rsp_i")));
        mem.get_port("ok_o")
            .connect(&driver.get_port("ok_i").bit(k));
        mem.get_port("count_o").unused();
    }
    top
}
