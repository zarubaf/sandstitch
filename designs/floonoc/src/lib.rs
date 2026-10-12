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

use sha2::{Digest, Sha256};
use topstitch::{ImportError, ModDef, ParameterValue, ParserConfig};

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

/// Writes [`cfg_pkg`] of `cfg` to a file named after the package and the
/// hash of its text, under `sandstitch-floonoc` in the system temporary
/// directory, and returns its path. The file is kept: the import, its
/// specialization key and the build manifest refer to it. Equal
/// configurations share one file; it is written atomically.
///
/// Panics if the file cannot be written.
pub fn cfg_pkg_file(cfg: &FlooConfig) -> PathBuf {
    let text = cfg_pkg(cfg);
    let digest = Sha256::digest(text.as_bytes());
    let hash: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join("sandstitch-floonoc");
    let path = dir.join(format!("{}-{hash}.sv", cfg.pkg_name()));
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == text) {
        return path;
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(
            ".{}-{hash}.{}.{:?}.tmp",
            cfg.pkg_name(),
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&tmp, &text)?;
        std::fs::rename(&tmp, &path)
    };
    write().unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    path
}

/// The router's parameter overrides for `cfg`: the link, header and node ID
/// types and the AXI configurations from the configuration package, XY
/// routing, and the port count and FIFO depths.
fn overrides(cfg: &FlooConfig) -> Vec<(&'static str, ParameterValue)> {
    let pkg = cfg.pkg_name();
    let ty = |name: &str| ParameterValue::Type(format!("{pkg}::{name}"));
    let expr = |name: &str| ParameterValue::Expr(format!("{pkg}::{name}"));
    vec![
        ("AxiCfgN", expr("AxiCfgN")),
        ("AxiCfgW", expr("AxiCfgW")),
        ("RouteAlgo", expr("RouteAlgo")),
        ("NumRoutes", cfg.num_routes.into()),
        ("InFifoDepth", cfg.in_fifo_depth.into()),
        ("OutFifoDepth", cfg.out_fifo_depth.into()),
        ("WideRwDecouple", expr("WideRwDecouple")),
        ("VcImpl", expr("VcImpl")),
        ("id_t", ty("id_t")),
        ("hdr_t", ty("hdr_t")),
        ("floo_req_t", ty("floo_req_t")),
        ("floo_rsp_t", ty("floo_rsp_t")),
        ("floo_wide_t", ty("floo_wide_t")),
    ]
}

/// `floo_nw_router` specialized for `cfg`, imported directly with its
/// overrides (its defaults do not elaborate). The configuration package
/// ([`cfg_pkg_file`]) is one of its sources, so it is in the build manifest
/// of any design that instantiates the router.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]).
#[track_caller]
pub fn router(cfg: &FlooConfig) -> Result<ModDef, ImportError> {
    let root = third_party();
    let path = |rel: &str| root.join(rel).to_str().unwrap().to_string();
    let pkg_file = cfg_pkg_file(cfg);
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
    let overrides = overrides(cfg);
    ModDef::try_from_verilog_parameterized("floo_nw_router", &parser_cfg, &overrides)
}
