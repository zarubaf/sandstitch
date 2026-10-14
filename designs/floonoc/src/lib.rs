// SPDX-License-Identifier: Apache-2.0

//! FlooNoC design package: the narrow/wide router `floo_nw_router`,
//! specialized for a [`FlooConfig`], and a 2x2 XY mesh of it.
//!
//! The RTL comes from the pinned sources in `third_party/` (FlooNoC,
//! `common_cells`, `axi`). Everything FlooNoC-specific lives here; the
//! library only provides generic import, specialization and emission.

mod config;
mod mesh;

use std::path::{Path, PathBuf};

use topstitch::{ImportError, ModDef, PackageDef, ParameterValue, ParserConfig};

pub use config::{AxiCfg, FlooConfig, VcImpl, WideRwDecouple, cfg_pkg};
pub use mesh::{Direction, mesh_2x2};

/// `third_party/` of this repository.
fn third_party() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party")
}

/// The sources of `floo_nw_router` in compile order, packages first; the
/// configuration package goes after `floo_pkg`.
const PACKAGES: &[&str] = &[
    "common_cells/src/cc_pkg.sv",
    "axi/src/axi_pkg.sv",
    "floonoc/hw/floo_pkg.sv",
];

const MODULES: &[&str] = &[
    "common_cells/src/cc_fifo.sv",
    "common_cells/src/cc_stream_fifo.sv",
    "common_cells/src/cc_stream_fifo_optimal_wrap.sv",
    "common_cells/src/cc_spill_register_flushable.sv",
    "common_cells/src/cc_spill_register.sv",
    "common_cells/src/cc_addr_decode_dync.sv",
    "common_cells/src/cc_addr_decode.sv",
    "common_cells/src/cc_popcount.sv",
    "common_cells/src/cc_lzc.sv",
    "common_cells/src/cc_stream_demux.sv",
    "common_cells/src/cc_stream_mux.sv",
    "common_cells/src/cc_stream_fork.sv",
    "common_cells/src/cc_stream_join_dynamic.sv",
    "common_cells/src/cc_rr_arb_tree.sv",
    "common_cells/src/cc_stream_arbiter.sv",
    "common_cells/src/cc_credit_counter.sv",
    "floonoc/hw/floo_route_xymask.sv",
    "floonoc/hw/floo_route_select.sv",
    "floonoc/hw/floo_reduction_sync.sv",
    "floonoc/hw/floo_reduction_unit.sv",
    "floonoc/hw/floo_reduction_arbiter.sv",
    "floonoc/hw/floo_wormhole_arbiter.sv",
    "floonoc/hw/floo_vc_arbiter.sv",
    "floonoc/hw/floo_output_arbiter.sv",
    "floonoc/hw/floo_router.sv",
    "floonoc/hw/floo_nw_router.sv",
];

const INCDIRS: &[&str] = &["common_cells/include", "axi/include", "floonoc/hw/include"];

/// Writes [`cfg_pkg`] of `cfg` to `out_dir`, in a file named after the
/// package and the hash of its text, and returns its path. The import, its
/// specialization key and the build manifest refer to the file, so
/// `out_dir` must outlive every use of the router and of designs built from
/// it. Equal configurations share one file; it is written atomically.
///
/// Panics if the file cannot be written.
pub fn cfg_pkg_file(cfg: &FlooConfig, out_dir: &Path) -> PathBuf {
    write_pkg(&cfg_pkg(cfg), out_dir)
}

#[track_caller]
fn write_pkg(pkg: &PackageDef, out_dir: &Path) -> PathBuf {
    pkg.write(out_dir).unwrap_or_else(|e| {
        panic!(
            "writing the configuration package to {}: {e}",
            out_dir.display()
        )
    })
}

/// The router's parameter overrides for `cfg`: the link, header and node ID
/// types and the AXI configurations from the configuration package, XY
/// routing, and the port count and FIFO depths.
#[track_caller]
fn overrides(cfg: &FlooConfig, pkg: &PackageDef) -> Vec<(&'static str, ParameterValue)> {
    vec![
        ("AxiCfgN", pkg.constant("AxiCfgN")),
        ("AxiCfgW", pkg.constant("AxiCfgW")),
        ("RouteAlgo", pkg.constant("RouteAlgo")),
        ("NumRoutes", cfg.num_routes.into()),
        ("InFifoDepth", cfg.in_fifo_depth.into()),
        ("OutFifoDepth", cfg.out_fifo_depth.into()),
        ("WideRwDecouple", pkg.constant("WideRwDecouple")),
        ("VcImpl", pkg.constant("VcImpl")),
        ("id_t", pkg.ty("id_t")),
        ("hdr_t", pkg.ty("hdr_t")),
        ("floo_req_t", pkg.ty("floo_req_t")),
        ("floo_rsp_t", pkg.ty("floo_rsp_t")),
        ("floo_wide_t", pkg.ty("floo_wide_t")),
    ]
}

/// `floo_nw_router` specialized for `cfg`, imported directly with its
/// overrides (its defaults do not elaborate). The configuration package
/// is written to `out_dir` ([`cfg_pkg_file`]) and is one of its sources, so
/// it is in the build manifest of any design that instantiates the router.
///
/// Each route `d` is the interface `link<d>` ([`link`]): the `req`, `rsp`
/// and `wide` channels, each in both directions (`floo_<ch>_i[d]` and
/// `floo_<ch>_o[d]`).
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]) or the package
/// cannot be written.
#[track_caller]
pub fn router(cfg: &FlooConfig, out_dir: &Path) -> Result<ModDef, ImportError> {
    let root = third_party();
    let path = |rel: &str| root.join(rel).to_str().unwrap().to_string();
    let pkg = cfg_pkg(cfg);
    let pkg_file = write_pkg(&pkg, out_dir);
    let sources: Vec<String> = PACKAGES
        .iter()
        .map(|rel| path(rel))
        .chain([pkg_file.to_str().unwrap().to_string()])
        .chain(MODULES.iter().map(|rel| path(rel)))
        .collect();
    let incdirs: Vec<String> = INCDIRS.iter().map(|rel| path(rel)).collect();
    let sources: Vec<&str> = sources.iter().map(String::as_str).collect();
    let incdirs: Vec<&str> = incdirs.iter().map(String::as_str).collect();
    let parser_cfg = ParserConfig {
        sources: &sources,
        incdirs: &incdirs,
        ..Default::default()
    };
    let overrides = overrides(cfg, &pkg);
    let router = ModDef::try_from_verilog_parameterized("floo_nw_router", &parser_cfg, &overrides)?;
    for d in 0..cfg.num_routes as usize {
        router.def_intf_members(
            link(d),
            CHANNELS.into_iter().flat_map(|ch| {
                ["i", "o"].map(|dir| (ch, router.get_port(format!("floo_{ch}_{dir}")).index(d)))
            }),
        );
    }
    Ok(router)
}

/// The router channels; each link carries each of them in both directions.
const CHANNELS: [&str; 3] = ["req", "rsp", "wide"];

/// The name of the router interface of route `d`.
pub fn link(d: usize) -> String {
    format!("link{d}")
}
