// SPDX-License-Identifier: Apache-2.0

//! The configuration and system packages, and compute and memory tiles that
//! elaborate in Slang from their build manifests.

use floonoc::*;

use topstitch::{EmitOptions, ModDef};

/// A width truncation of the kind the vendored RTL has, for the emitted RTL.
const OWN_TRUNCATION: &str = "  logic [3:0] wide;\n  logic [1:0] narrowed;\n  \
                              assign wide = '1;\n  assign narrowed = wide;\n";

#[path = "../../../tests/common/slang.rs"]
#[allow(dead_code)]
mod slang;
use slang::elaborate_manifest_vendored;

/// The vendored FlooNoC RTL, whose own implicit width conversions Slang
/// reports as warnings with every configuration (also with the
/// `ChimneyDefaultCfg` network interface of the upstream examples); the
/// emitted tiles and the fixtures stay strict.
fn floonoc_hw() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("third_party/floonoc/hw")
}

/// `floo_synth_nw_pkg` on 2x2-bit coordinates at address bits 16 and 20.
fn cfg() -> FlooConfig {
    FlooConfig::synth("tile")
}

/// Memories at (1, 0) and (1, 1); a one-core tile at (0, 0) and a two-core
/// tile at (0, 1) whose cores target different memories.
fn spec() -> MeshSpec {
    let mut spec = MeshSpec::new(2, 2);
    spec.set(1, 0, Endpoint::Memory)
        .set(1, 1, Endpoint::Memory)
        .set(
            0,
            0,
            Endpoint::Compute {
                targets: vec![Target {
                    x: 1,
                    y: 1,
                    seed: 3,
                }],
            },
        )
        .set(
            0,
            1,
            Endpoint::Compute {
                targets: vec![
                    Target {
                        x: 1,
                        y: 0,
                        seed: 5,
                    },
                    Target {
                        x: 1,
                        y: 1,
                        seed: 1023,
                    },
                ],
            },
        );
    spec
}

fn targets(spec: &MeshSpec, x: usize, y: usize) -> Vec<Target> {
    match spec.endpoint(x, y) {
        Endpoint::Compute { targets } => targets.clone(),
        e => panic!("({x}, {y}) is {e:?}"),
    }
}

/// Emits `tile`, elaborates it in Slang from its manifest and returns the
/// emitted text and the manifest's sources.
fn elaborate(tile: &ModDef) -> (String, Vec<std::path::PathBuf>) {
    let emitted = tile.emit(EmitOptions::default());
    let manifest = tile.manifest();
    if let Err(slang) =
        elaborate_manifest_vendored(&manifest, &emitted, &tile.get_name(), &floonoc_hw())
    {
        panic!("Slang rejects the emitted tile:\n{emitted}\n{slang}");
    }
    (emitted, manifest.sources)
}

fn width(tile: &ModDef, port: &str) -> usize {
    tile.get_port(port).io().width()
}

#[test]
fn one_core_compute_tile_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = (cfg(), spec());
    let sys = sys_pkg(&cfg, &spec);
    let tile = compute_tile(
        &cfg,
        &sys,
        "tile_compute_x0_y0",
        &targets(&spec, 0, 0),
        dir.path(),
    );
    let (emitted, sources) = elaborate(&tile);

    // Both packages and the core fixture are manifest sources.
    let cfg_file = cfg_pkg_file(&cfg, dir.path());
    let sys_file = sys.write(dir.path()).unwrap();
    for file in [&cfg_file, &sys_file] {
        assert!(sources.contains(file), "{file:?} not in {sources:#?}");
    }
    assert!(sources.iter().any(|s| s.ends_with("rtl/traffic_core.sv")));
    assert!(
        sources
            .iter()
            .any(|s| s.ends_with("floonoc/hw/floo_nw_chimney.sv"))
    );

    // The core drives the network interface directly with the narrow
    // manager types, and targets memory (1, 1) through the system package.
    assert!(!emitted.contains("axi_mux"), "{emitted}");
    for param in [
        ".axi_req_t(tile_pkg::axi_narrow_in_req_t)",
        ".TargetBase(tile_sys_pkg::MemBase_x1_y1)",
        ".ChimneyCfgN(tile_pkg::ChimneyCfgNCompute)",
        ".ChimneyCfgW(tile_pkg::ChimneyCfgW)",
        ".RouteCfg(tile_pkg::RouteCfg)",
        ".sam_rule_t(tile_pkg::sam_rule_t)",
    ] {
        assert!(emitted.contains(param), "{param} in\n{emitted}");
    }
    assert_eq!(width(&tile, "done_o"), 1);
    assert_eq!(width(&tile, "error_o"), 1);
    // The link is the router's: req 116, rsp 100 and wide 1204 bits.
    for (port, bits) in [
        ("floo_req_i", 116),
        ("floo_req_o", 116),
        ("floo_rsp_i", 100),
        ("floo_rsp_o", 100),
        ("floo_wide_i", 1204),
        ("floo_wide_o", 1204),
    ] {
        assert_eq!(width(&tile, port), bits, "{port}");
    }
    assert_eq!(width(&tile, "id_i"), 5);
    assert_eq!(tile.get_intf(LINK).members().len(), 6);
}

#[test]
fn two_core_compute_tile_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = (cfg(), spec());
    let sys = sys_pkg(&cfg, &spec);
    let tile = compute_tile(
        &cfg,
        &sys,
        "tile_compute_x0_y1",
        &targets(&spec, 0, 1),
        dir.path(),
    );
    let (emitted, _) = elaborate(&tile);

    // The cores use the system package's two-core types, whose ID is one
    // bit narrower than the network interface's, and meet in an axi_mux.
    for param in [
        ".axi_req_t(tile_sys_pkg::core2_req_t)",
        ".slv_req_t(tile_sys_pkg::core2_req_t)",
        ".mst_req_t(tile_pkg::axi_narrow_in_req_t)",
        ".TargetBase(tile_sys_pkg::MemBase_x1_y0)",
        ".TargetBase(tile_sys_pkg::MemBase_x1_y1)",
    ] {
        assert!(emitted.contains(param), "{param} in\n{emitted}");
    }
    assert_eq!(emitted.matches(") i_axi_mux (").count(), 1);
    assert_eq!(core_id_width(&cfg, 2), Ok(cfg.narrow.in_id_width - 1));
    assert_eq!(width(&tile, "done_o"), 2);
    assert_eq!(width(&tile, "error_o"), 2);
}

#[test]
fn memory_tile_elaborates() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg();
    let tile = memory_tile(&cfg, "tile_memory", dir.path());
    let (emitted, sources) = elaborate(&tile);
    assert!(sources.contains(&cfg_pkg_file(&cfg, dir.path())));
    assert!(emitted.contains(".ChimneyCfgN(tile_pkg::ChimneyCfgNMem)"));
    for (port, ty) in [
        ("axi_req_o", "tile_pkg::axi_narrow_out_req_t"),
        ("axi_rsp_i", "tile_pkg::axi_narrow_out_rsp_t"),
    ] {
        let found = tile.get_port(port).port_type().unwrap().to_string();
        assert!(found.contains(ty), "{port}: {found}");
    }
}

#[test]
fn system_package_holds_the_address_map_and_core_types() {
    let (cfg, spec) = (cfg(), spec());
    let sys = sys_pkg(&cfg, &spec);
    let text = sys.emit();
    // Windows sit at the coordinates' address bits: x at 16, y at 20.
    assert_eq!(window_base(&cfg, 1, 0), 0x1_0000);
    assert_eq!(window_base(&cfg, 1, 1), 0x11_0000);
    assert!(
        text.contains("localparam int unsigned MemBase_x1_y0 = 65536;"),
        "{text}"
    );
    assert!(
        text.contains("localparam int unsigned MemBase_x1_y1 = 1114112;"),
        "{text}"
    );
    assert!(!text.contains("MemBase_x0"), "{text}");
    // Only the two-core tile needs its own types.
    assert!(text.contains("typedef logic [2:0] core2_id_t;"), "{text}");
    assert!(!text.contains("core1"), "{text}");
    let core2 = "`AXI_TYPEDEF_ALL_CT(core2, core2_req_t, core2_rsp_t, \
                 tile_pkg::axi_narrow_addr_t, core2_id_t";
    assert!(text.contains(core2), "{text}");
    sys.ty("core2_aw_chan_t");
    sys.constant("MemBase_x1_y1");
}

#[test]
fn configuration_package_holds_the_network_interface_configuration() {
    let text = cfg_pkg(&cfg()).emit();
    for line in [
        "localparam floo_pkg::route_cfg_t RouteCfg = '{RouteAlgo: floo_pkg::XYRouting, \
         UseIdTable: 0, XYAddrOffsetX: 16, XYAddrOffsetY: 20, IdAddrOffset: 0, \
         NumSamRules: 0, NumRoutes: 0, CollectiveCfg: floo_pkg::CollectiveDefaultCfg};",
        "localparam floo_pkg::chimney_cfg_t ChimneyCfgW = floo_pkg::ChimneyDefaultCfg;",
        "typedef logic [0:0] sam_rule_t;",
    ] {
        assert!(text.contains(line), "{line} in\n{text}");
    }
    for (name, sbr, mgr) in [("ChimneyCfgNCompute", 0, 1), ("ChimneyCfgNMem", 1, 0)] {
        let decl = format!(
            "localparam floo_pkg::chimney_cfg_t {name} = '{{EnSbrPort: {sbr}, EnMgrPort: {mgr}, "
        );
        assert!(text.contains(&decl), "{decl} in\n{text}");
    }
    let pkg = cfg_pkg(&cfg());
    for t in [
        "axi_narrow_in_req_t",
        "axi_wide_out_rsp_t",
        "axi_narrow_in_aw_chan_t",
        "axi_narrow_addr_t",
    ] {
        pkg.ty(t);
    }
}

#[test]
fn address_bits_and_core_counts_are_validated() {
    let check = |f: &dyn Fn(&mut FlooConfig), msg: &str| {
        let mut cfg = cfg();
        f(&mut cfg);
        let err = cfg.validate().unwrap_err();
        assert!(err.contains(msg), "{err}");
    };
    check(&|c| c.addr_offset_y = 17, "overlap the Y coordinate");
    check(
        &|c| {
            c.narrow.addr_width = 21;
            c.wide.addr_width = 21;
        },
        "above narrow.addr_width 21",
    );
    check(&|c| c.addr_offset_y = 31, "above 32");
    check(&|c| c.vc_impl = VcImpl::VcCredit, "VcCredit is unsupported");
    check(&|c| c.wide.addr_width = 32, "differs from wide.addr_width");
    check(&|c| c.addr_offset_x = 15, "addr_offset_x 15 is below 16");
    let mut cfg = cfg();
    cfg.addr_offset_x = 18;
    assert_eq!(cfg.validate(), Ok(()), "X bits 18..20 end where Y begins");

    // Four input ID bits: up to eight cores keep one ID bit each.
    let cfg = self::cfg();
    assert_eq!(core_id_width(&cfg, 8), Ok(1));
    assert!(core_id_width(&cfg, 9).unwrap_err().contains("no ID bit"));
}

#[test]
#[should_panic(expected = "seed 1024 is not below 1024")]
fn compute_tile_rejects_large_seed() {
    let dir = tempfile::tempdir().unwrap();
    let (cfg, spec) = (cfg(), spec());
    let sys = sys_pkg(&cfg, &spec);
    let target = Target {
        x: 1,
        y: 0,
        seed: 1024,
    };
    compute_tile(&cfg, &sys, "bad", &[target], dir.path());
}

/// Only the vendored RTL's width conversions are tolerated: the same one in
/// the emitted RTL, in the hierarchy of a tile whose vendored warnings
/// pass, fails.
#[test]
fn own_width_warning_fails_next_to_vendored_ones() {
    let dir = tempfile::tempdir().unwrap();
    let tile = memory_tile(&cfg(), "tile_memory", dir.path());
    let top = ModDef::new("warn_top");
    let inst = top.instantiate(&tile, Some("i_tile"), None);
    for port in inst.get_ports(None) {
        port.export();
    }
    let emitted = top.emit(EmitOptions::default());
    let manifest = top.manifest();
    elaborate_manifest_vendored(&manifest, &emitted, "warn_top", &floonoc_hw())
        .unwrap_or_else(|e| panic!("the wrapper alone elaborates:\n{e}"));

    let start = emitted.find("module warn_top").unwrap();
    let end = start + emitted[start..].find("endmodule").unwrap();
    let warned = format!("{}{OWN_TRUNCATION}{}", &emitted[..end], &emitted[end..]);
    let err = elaborate_manifest_vendored(&manifest, &warned, "warn_top", &floonoc_hw())
        .expect_err("a width truncation in the emitted RTL fails");
    assert!(err.contains("truncates from 4 to 2 bits"), "{err}");
}
