// SPDX-License-Identifier: Apache-2.0

//! The synthesizable system: a mesh with its endpoints and flat core
//! status vectors.

use std::path::Path;

use topstitch::{IO, ModDef};

use crate::{Endpoint, FlooConfig, MeshSpec, Target, mesh};

/// The small configuration (doc/design/reference-noc.md, decision 8):
/// `floo_synth_nw_pkg`'s router configuration named `small`, and a 2x2
/// mesh with one memory at (1, 1) and single-core compute tiles at the
/// other three routers, all targeting the memory with distinct seeds. The
/// core at (0, 0) is two hops from the memory.
pub fn small() -> (FlooConfig, MeshSpec) {
    let cfg = FlooConfig::synth("small");
    let mut spec = MeshSpec::new(2, 2);
    spec.set(1, 1, Endpoint::Memory);
    for (seed, (x, y)) in [(0, 0), (0, 1), (1, 0)].into_iter().enumerate() {
        let target = Target {
            x: 1,
            y: 1,
            seed: seed as u32 + 1,
        };
        spec.set(
            x,
            y,
            Endpoint::Compute {
                targets: vec![target],
            },
        );
    }
    (cfg, spec)
}

/// The large configuration (doc/design/reference-noc.md, decision 8):
/// `floo_synth_nw_pkg`'s router configuration named `large`, and a 3x3 mesh
/// with memories at (2, 0) and (0, 2) and a two-core compute tile at each
/// of the other seven routers. The two cores of a tile target different
/// memories, and every core has a distinct seed. The tile at (0, 0) is
/// two hops from each memory.
pub fn large() -> (FlooConfig, MeshSpec) {
    let cfg = FlooConfig::synth("large");
    let memories = [(2, 0), (0, 2)];
    let mut spec = MeshSpec::new(3, 3);
    for (x, y) in memories {
        spec.set(x, y, Endpoint::Memory);
    }
    let tiles: Vec<(usize, usize)> = spec
        .routers()
        .filter(|&(x, y)| !memories.contains(&(x, y)))
        .collect();
    for (i, &(x, y)) in tiles.iter().enumerate() {
        let targets = memories
            .iter()
            .enumerate()
            .map(|(m, &(mx, my))| Target {
                x: mx,
                y: my,
                seed: (2 * i + m) as u32 + 1,
            })
            .collect();
        spec.set(x, y, Endpoint::Compute { targets });
    }
    (cfg, spec)
}

/// The name of the system module of `cfg`: `<name>_system`.
pub(crate) fn system_name(cfg: &FlooConfig) -> String {
    format!("{}_system", cfg.name)
}

/// The synthesizable system `<name>_system` of `cfg` and `spec`: the
/// [`mesh`] as `i_mesh`, with test enable tied low.
///
/// Ports:
/// - `clk_i` and `rst_ni`;
/// - `done_o[N]` and `error_o[N]` of all N cores, bit `i` from core `i` of
///   [`MeshSpec::cores`];
/// - every memory's AXI subordinate port, `x<X>_y<Y>_axi_req_o` and
///   `x<X>_y<Y>_axi_rsp_i`;
/// - every exported local link, under the mesh's names.
///
/// The packages are written to `out_dir`.
///
/// Panics if `spec` has no core, or as [`mesh`] does.
#[track_caller]
pub fn system(cfg: &FlooConfig, spec: &MeshSpec, out_dir: &Path) -> ModDef {
    let cores = spec.cores().len();
    assert!(cores > 0, "{}: a system needs at least one core", cfg.name);
    let mesh = mesh(cfg, spec, out_dir);
    let top = ModDef::new(system_name(cfg));
    let inst = top.instantiate(&mesh, Some("i_mesh"), None);
    inst.get_port("test_enable_i").tieoff(0);
    let done = top.add_port("done_o", IO::Output(cores));
    let error = top.add_port("error_o", IO::Output(cores));
    let mut status = Vec::new();
    let mut lsb = 0;
    for (x, y) in spec.routers() {
        let Endpoint::Compute { targets } = spec.endpoint(x, y) else {
            continue;
        };
        let msb = lsb + targets.len() - 1;
        for (port, vector) in [("done_o", &done), ("error_o", &error)] {
            let name = format!("x{x}_y{y}_{port}");
            inst.get_port(&name).connect(&vector.slice(msb, lsb));
            status.push(name);
        }
        lsb = msb + 1;
    }
    for port in inst.get_ports(None) {
        let name = port.name();
        if name != "test_enable_i" && !status.iter().any(|s| s == name) {
            port.export();
        }
    }
    top
}
