// SPDX-License-Identifier: Apache-2.0

//! The network interface `floo_nw_chimney` and the compute and memory tiles
//! built around it.

use std::path::{Path, PathBuf};

use topstitch::{ConvertibleToPortSlice, IO, ImportError, ModDef, PackageDef, ParameterValue};

use crate::spec::{core_axi, core_id_width, mem_base_name};
use crate::{CHANNELS, FlooConfig, LIBDIRS, Target, cfg_pkg, import, rtl, third_party, write_pkg};

/// Which narrow AXI side of a network interface is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChimneyRole {
    /// An AXI manager (traffic cores) issues requests into the network:
    /// `ChimneyCfgNCompute`.
    Compute,
    /// An AXI subordinate (a memory) serves requests from the network:
    /// `ChimneyCfgNMem`.
    Memory,
}

/// The name of the network interface's link to its router.
pub const LINK: &str = "link";

/// `floo_nw_chimney` specialized for `cfg` and `role`, imported directly
/// with its overrides, all from the configuration package (written to
/// `out_dir`): the AXI configurations and types, the narrow chimney
/// configuration of `role`, `ChimneyCfgW` for the wide side, `RouteCfg`
/// (address-based XY routing), `sam_rule_t`, and the node ID, header and
/// link types. `AtopSupport` (on) and `MaxAtomicTxns` (1) keep their
/// defaults, which satisfy `MaxAtomicTxns <= 2**OutIdWidth - 1` for every
/// valid configuration.
///
/// Its link to the router is the interface [`LINK`]: the `req`, `rsp` and
/// `wide` channels, each in both directions (`floo_<ch>_i`, `floo_<ch>_o`).
/// The caller ties `test_enable_i`, `sram_cfg_i` and `route_table_i`.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]) or the package
/// cannot be written.
#[track_caller]
pub fn chimney(cfg: &FlooConfig, role: ChimneyRole, out_dir: &Path) -> Result<ModDef, ImportError> {
    let pkg = cfg_pkg(cfg);
    let pkg_file = write_pkg(&pkg, out_dir);
    let narrow_cfg = match role {
        ChimneyRole::Compute => "ChimneyCfgNCompute",
        ChimneyRole::Memory => "ChimneyCfgNMem",
    };
    let mut types: Vec<String> = Vec::new();
    for side in ["narrow", "wide"] {
        for dir in ["in", "out"] {
            for msg in ["req", "rsp"] {
                types.push(format!("axi_{side}_{dir}_{msg}_t"));
            }
        }
    }
    types.extend(CHANNELS.map(|ch| format!("floo_{ch}_t")));
    let mut overrides: Vec<(&str, ParameterValue)> = vec![
        ("AxiCfgN", pkg.constant("AxiCfgN")),
        ("AxiCfgW", pkg.constant("AxiCfgW")),
        ("ChimneyCfgN", pkg.constant(narrow_cfg)),
        ("ChimneyCfgW", pkg.constant("ChimneyCfgW")),
        ("RouteCfg", pkg.constant("RouteCfg")),
        ("WideRwDecouple", pkg.constant("WideRwDecouple")),
        ("VcImpl", pkg.constant("VcImpl")),
        ("id_t", pkg.ty("id_t")),
        ("hdr_t", pkg.ty("hdr_t")),
        ("sam_rule_t", pkg.ty("sam_rule_t")),
    ];
    overrides.extend(types.iter().map(|t| (t.as_str(), pkg.ty(t))));
    let chimney = import(
        "floo_nw_chimney",
        &[pkg_file],
        &[third_party().join("floonoc/hw/floo_nw_chimney.sv")],
        LIBDIRS,
        &overrides,
    )?;
    chimney.def_intf_members(
        LINK,
        CHANNELS.into_iter().flat_map(|ch| {
            ["i", "o"].map(|dir| {
                (
                    ch,
                    chimney.get_port(format!("floo_{ch}_{dir}")).to_port_slice(),
                )
            })
        }),
    );
    Ok(chimney)
}

/// `traffic_core` (`rtl/traffic_core.sv`) with the request and response
/// types `<prefix>_req_t` and `<prefix>_rsp_t` of `axi_pkg`, targeting the
/// memory window `sys.constant(MemBase_x<X>_y<Y>)` with `target.seed`.
/// `pkg_files` are the generated packages it needs, in order.
///
/// Panics if `sys` has no memory at the target.
#[track_caller]
fn traffic_core(
    sys: &PackageDef,
    axi_pkg: &PackageDef,
    prefix: &str,
    target: Target,
    pkg_files: &[PathBuf],
) -> Result<ModDef, ImportError> {
    let overrides = [
        ("axi_req_t", axi_pkg.ty(&format!("{prefix}_req_t"))),
        ("axi_rsp_t", axi_pkg.ty(&format!("{prefix}_rsp_t"))),
        (
            "TargetBase",
            sys.constant(&mem_base_name(target.x, target.y)),
        ),
        ("Seed", target.seed.into()),
    ];
    import(
        "traffic_core",
        pkg_files,
        &[rtl().join("traffic_core.sv")],
        &[],
        &overrides,
    )
}

/// `axi_mux` joining `n` cores with the core AXI types of `sys`
/// (`core<n>_*`) onto the narrow manager types of `cfg_pkg`.
#[track_caller]
fn axi_mux(
    cfg_pkg: &PackageDef,
    sys: &PackageDef,
    n: usize,
    id_width: u32,
    pkg_files: &[PathBuf],
) -> Result<ModDef, ImportError> {
    let core = core_axi(n);
    let slv = |t: &str| sys.ty(&format!("{core}_{t}_t"));
    let mst = |t: &str| cfg_pkg.ty(&format!("axi_narrow_in_{t}_t"));
    let overrides = [
        ("SlvAxiIDWidth", id_width.into()),
        ("slv_aw_chan_t", slv("aw_chan")),
        ("mst_aw_chan_t", mst("aw_chan")),
        ("w_chan_t", mst("w_chan")),
        ("slv_b_chan_t", slv("b_chan")),
        ("mst_b_chan_t", mst("b_chan")),
        ("slv_ar_chan_t", slv("ar_chan")),
        ("mst_ar_chan_t", mst("ar_chan")),
        ("slv_r_chan_t", slv("r_chan")),
        ("mst_r_chan_t", mst("r_chan")),
        ("slv_req_t", slv("req")),
        ("slv_resp_t", slv("rsp")),
        ("mst_req_t", mst("req")),
        ("mst_resp_t", mst("rsp")),
        ("NoSlvPorts", (n as u32).into()),
    ];
    import(
        "axi_mux",
        pkg_files,
        &[third_party().join("axi/src/axi_mux.sv")],
        LIBDIRS,
        &overrides,
    )
}

/// Ties the network interface inputs nobody drives and marks its unused
/// outputs: test enable, SRAM configuration, routing table, and the whole
/// wide AXI side (the tiles move only narrow traffic).
fn tie_chimney(chim: &topstitch::ModInst) {
    for port in ["test_enable_i", "sram_cfg_i", "route_table_i"] {
        chim.get_port(port).tieoff(0);
    }
    chim.get_port("axi_wide_in_req_i").tieoff(0);
    chim.get_port("axi_wide_in_rsp_o").unused();
    chim.get_port("axi_wide_out_req_o").unused();
    chim.get_port("axi_wide_out_rsp_i").tieoff(0);
}

/// Adds clock and reset to `tile`, connects them to `insts`, and exports
/// the network interface's node ID (`id_i`) and its link ([`LINK`], ports
/// `floo_<ch>_{i,o}`).
fn tile_ports(tile: &ModDef, chim: &topstitch::ModInst, insts: &[&topstitch::ModInst]) {
    let clk = tile.add_port("clk_i", IO::Input(1));
    let rst = tile.add_port("rst_ni", IO::Input(1));
    for inst in insts.iter().copied().chain([chim]) {
        clk.connect(&inst.get_port("clk_i"));
        rst.connect(&inst.get_port("rst_ni"));
    }
    chim.get_port("id_i").export_as("id_i");
    chim.get_intf(LINK).export_with_prefix(LINK, "floo_");
}

/// A compute tile named `name`: one `traffic_core` per target, each
/// targeting the base address `MemBase_x<X>_y<Y>` of `sys` ([`sys_pkg`]
/// of the same configuration), and one network interface in the
/// [`ChimneyRole::Compute`] role. A single core drives the network
/// interface directly; more cores go through an `axi_mux`, with the core
/// AXI types of `sys`.
///
/// Ports: `clk_i`, `rst_ni`, the node ID `id_i`, the router link
/// [`LINK`] (`floo_<ch>_{i,o}`), and `done_o[N]` and `error_o[N]`, bit `i`
/// from core `i`. The packages are written to `out_dir`.
///
/// Panics if `cfg` is invalid, `targets` is empty, a seed is not below
/// 1024, the core count leaves no ID bit, `sys` lacks a target memory or
/// the core types, or an import fails.
///
/// [`sys_pkg`]: crate::sys_pkg
#[track_caller]
pub fn compute_tile(
    cfg: &FlooConfig,
    sys: &PackageDef,
    name: &str,
    targets: &[Target],
    out_dir: &Path,
) -> ModDef {
    let n = targets.len();
    assert!(n > 0, "{name}: a compute tile needs at least one target");
    for t in targets {
        assert!(t.seed < 1024, "{name}: seed {} is not below 1024", t.seed);
    }
    let id_width = core_id_width(cfg, n).unwrap_or_else(|e| panic!("{e}"));
    let pkg = cfg_pkg(cfg);
    let pkg_files = [write_pkg(&pkg, out_dir), write_pkg(sys, out_dir)];
    let chim_def = chimney(cfg, ChimneyRole::Compute, out_dir).unwrap_or_else(|e| panic!("{e}"));

    let tile = ModDef::new(name);
    let chim = tile.instantiate(&chim_def, Some("i_chimney"), None);
    tie_chimney(&chim);
    chim.get_port("axi_narrow_out_req_o").unused();
    chim.get_port("axi_narrow_out_rsp_i").tieoff(0);

    let (core_pkg, core_prefix) = if n == 1 {
        (&pkg, "axi_narrow_in".to_string())
    } else {
        (sys, core_axi(n))
    };
    let done = tile.add_port("done_o", IO::Output(n));
    let error = tile.add_port("error_o", IO::Output(n));
    let mut insts = Vec::new();
    let mux = (n > 1).then(|| {
        let def = axi_mux(&pkg, sys, n, id_width, &pkg_files).unwrap_or_else(|e| panic!("{e}"));
        let mux = tile.instantiate(&def, Some("i_axi_mux"), None);
        mux.get_port("mst_req_o")
            .connect(&chim.get_port("axi_narrow_in_req_i"));
        mux.get_port("mst_resp_i")
            .connect(&chim.get_port("axi_narrow_in_rsp_o"));
        mux
    });
    for (i, target) in targets.iter().enumerate() {
        let def = traffic_core(sys, core_pkg, &core_prefix, *target, &pkg_files)
            .unwrap_or_else(|e| panic!("{e}"));
        let core = tile.instantiate(&def, Some(&format!("i_core{i}")), None);
        match &mux {
            Some(mux) => {
                core.get_port("axi_req_o")
                    .connect(&mux.get_port("slv_reqs_i").index(i));
                core.get_port("axi_rsp_i")
                    .connect(&mux.get_port("slv_resps_o").index(i));
            }
            None => {
                core.get_port("axi_req_o")
                    .connect(&chim.get_port("axi_narrow_in_req_i"));
                core.get_port("axi_rsp_i")
                    .connect(&chim.get_port("axi_narrow_in_rsp_o"));
            }
        }
        core.get_port("done_o").connect(&done.bit(i));
        core.get_port("error_o").connect(&error.bit(i));
        insts.push(core);
    }
    insts.extend(mux);
    tile_ports(&tile, &chim, &insts.iter().collect::<Vec<_>>());
    tile
}

/// A memory tile named `name`: one network interface in the
/// [`ChimneyRole::Memory`] role whose narrow AXI subordinate side is
/// exported as `axi_req_o` and `axi_rsp_i` for the memory.
///
/// Ports: `clk_i`, `rst_ni`, the node ID `id_i`, the router link
/// [`LINK`] (`floo_<ch>_{i,o}`), `axi_req_o` and `axi_rsp_i`. The
/// configuration package is written to `out_dir`.
///
/// Panics if `cfg` is invalid or the import fails.
#[track_caller]
pub fn memory_tile(cfg: &FlooConfig, name: &str, out_dir: &Path) -> ModDef {
    let chim_def = chimney(cfg, ChimneyRole::Memory, out_dir).unwrap_or_else(|e| panic!("{e}"));
    let tile = ModDef::new(name);
    let chim = tile.instantiate(&chim_def, Some("i_chimney"), None);
    tie_chimney(&chim);
    chim.get_port("axi_narrow_in_req_i").tieoff(0);
    chim.get_port("axi_narrow_in_rsp_o").unused();
    chim.get_port("axi_narrow_out_req_o").export_as("axi_req_o");
    chim.get_port("axi_narrow_out_rsp_i").export_as("axi_rsp_i");
    tile_ports(&tile, &chim, &[]);
    tile
}
