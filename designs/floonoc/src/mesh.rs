// SPDX-License-Identifier: Apache-2.0

//! An X×Y mesh of `floo_nw_router` with XY routing and the endpoints at its
//! local ports.

use std::path::Path;

use topstitch::{IO, ModDef, ModInst};

use crate::{
    Endpoint, FlooConfig, LINK, MeshSpec, compute_tile, link, memory_tile, router, sys_pkg,
};

/// The router port directions, `floo_pkg::route_direction_e`. Ports from
/// [`Direction::Eject`] on are local ports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Y increasing.
    North = 0,
    /// X increasing.
    East = 1,
    /// Y decreasing.
    South = 2,
    /// X decreasing.
    West = 3,
    /// The first local port.
    Eject = 4,
}

/// Connects the `dir` link of `a` to the `opposite` link of `b`: each
/// channel's output drives the other router's input, in both directions.
/// Each link struct carries its own ready.
fn connect(a: &ModInst, dir: Direction, b: &ModInst, opposite: Direction) {
    a.get_intf(link(dir as usize))
        .connect(&b.get_intf(link(opposite as usize)), false);
}

/// Ties the node ID input `id_i` of `inst` (a router, a tile or a network
/// interface) to router (`x`, `y`), port ID 0.
pub fn tie_node_id(inst: &ModInst, x: usize, y: usize) {
    let id = inst.get_port("id_i");
    id.field("x").tieoff(x);
    id.field("y").tieoff(y);
    id.field("port_id").tieoff(0);
}

/// The name of the mesh module of `cfg` and `spec`:
/// `<name>_mesh_<X>x<Y>`.
pub fn mesh_name(cfg: &FlooConfig, spec: &MeshSpec) -> String {
    format!("{}_mesh_{}x{}", cfg.name, spec.nx(), spec.ny())
}

/// A mesh of `spec.nx()`×`spec.ny()` routers `i_router_x<X>_y<Y>`, named
/// [`mesh_name`]. Neighbours are linked East-West and North-South; each
/// router's `id_i` is tied to its coordinates (port ID 0). Mesh-edge links
/// are tied to zero (inputs) and marked unused (outputs). Clock, reset and
/// test enable are shared inputs. The routing table input and the
/// reduction offload interfaces are not used by XY routing without
/// collectives: inputs tied to zero, outputs unused.
///
/// The local links of each router (ports from [`Direction::Eject`] on) go
/// to its endpoint ([`Endpoint`]):
/// - `Export`: exported as `x<X>_y<Y>_p<P>_floo_<ch>_{i,o}`;
/// - `Tie`: tied to zero and marked unused;
/// - `Compute`: a [`compute_tile`] `<name>_compute_x<X>_y<Y>` instance
///   `i_compute_x<X>_y<Y>`, whose `done_o` and `error_o` are exported as
///   `x<X>_y<Y>_done_o` and `x<X>_y<Y>_error_o`;
/// - `Memory`: a [`memory_tile`] `<name>_memory` instance
///   `i_memory_x<X>_y<Y>`, whose AXI subordinate port is exported as
///   `x<X>_y<Y>_axi_req_o` and `x<X>_y<Y>_axi_rsp_i`.
///
/// A tile shares the router's clock and reset, its `id_i` is tied to the
/// router's coordinates and its link connects to the router's first local
/// link. The compute tiles target the windows of the system package
/// ([`sys_pkg`]) of `cfg` and `spec`.
///
/// The packages are written to `out_dir` ([`router`]).
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]), `spec` fails
/// [`MeshSpec::check`], a package cannot be written or an import fails.
#[track_caller]
pub fn mesh(cfg: &FlooConfig, spec: &MeshSpec, out_dir: &Path) -> ModDef {
    if let Err(e) = cfg.validate() {
        panic!("invalid FlooNoC configuration: {e}");
    }
    if let Err(e) = spec.check(cfg) {
        panic!("invalid mesh: {e}");
    }
    let router = router(cfg, out_dir).unwrap_or_else(|e| panic!("{e}"));
    let sys = (!spec.cores().is_empty()).then(|| sys_pkg(cfg, spec));
    let memory = (!spec.memories().is_empty())
        .then(|| memory_tile(cfg, &format!("{}_memory", cfg.name), out_dir));
    let (nx, ny) = (spec.nx(), spec.ny());
    let top = ModDef::new(mesh_name(cfg, spec));
    let clk = top.add_port("clk_i", IO::Input(1));
    let rst = top.add_port("rst_ni", IO::Input(1));
    let test = top.add_port("test_enable_i", IO::Input(1));

    let attach = |tile: &ModInst, r: &ModInst, x: usize, y: usize| {
        clk.connect(&tile.get_port("clk_i"));
        rst.connect(&tile.get_port("rst_ni"));
        tie_node_id(tile, x, y);
        tile.get_intf(LINK)
            .connect(&r.get_intf(link(Direction::Eject as usize)), false);
    };

    let mut routers = Vec::new();
    for x in 0..nx {
        let mut column = Vec::new();
        for y in 0..ny {
            let r = top.instantiate(&router, Some(&format!("i_router_x{x}_y{y}")), None);
            clk.connect(&r.get_port("clk_i"));
            rst.connect(&r.get_port("rst_ni"));
            test.connect(&r.get_port("test_enable_i"));
            tie_node_id(&r, x, y);
            r.get_port("id_route_map_i").tieoff(0);
            for side in ["wide", "narrow"] {
                r.get_port(format!("offload_{side}_rsp_i")).tieoff(0);
                r.get_port(format!("offload_{side}_req_o")).unused();
            }
            let locals = Direction::Eject as usize..cfg.num_routes as usize;
            let at = format!("x{x}_y{y}");
            match spec.endpoint(x, y) {
                Endpoint::Export => {
                    for p in locals {
                        let name = format!("{at}_p{p}");
                        r.get_intf(link(p))
                            .export_with_prefix(&name, format!("{name}_floo_"));
                    }
                }
                Endpoint::Tie => {
                    for p in locals {
                        r.get_intf(link(p)).unused_and_tieoff(0);
                    }
                }
                Endpoint::Compute { targets } => {
                    let sys = sys.as_ref().expect("a system package with cores");
                    let name = format!("{}_compute_{at}", cfg.name);
                    let def = compute_tile(cfg, sys, &name, targets, out_dir);
                    let tile = top.instantiate(&def, Some(&format!("i_compute_{at}")), None);
                    attach(&tile, &r, x, y);
                    for port in ["done_o", "error_o"] {
                        tile.get_port(port).export_as(format!("{at}_{port}"));
                    }
                }
                Endpoint::Memory => {
                    let def = memory.as_ref().expect("a memory tile");
                    let tile = top.instantiate(def, Some(&format!("i_memory_{at}")), None);
                    attach(&tile, &r, x, y);
                    for port in ["axi_req_o", "axi_rsp_i"] {
                        tile.get_port(port).export_as(format!("{at}_{port}"));
                    }
                }
            }
            column.push(r);
        }
        routers.push(column);
    }

    for pair in routers.windows(2) {
        for (west, east) in pair[0].iter().zip(&pair[1]) {
            connect(west, Direction::East, east, Direction::West);
        }
    }
    for column in &routers {
        for pair in column.windows(2) {
            connect(&pair[0], Direction::North, &pair[1], Direction::South);
        }
    }

    for (x, column) in routers.iter().enumerate() {
        for (y, r) in column.iter().enumerate() {
            let x_edges = [(x == 0, Direction::West), (x == nx - 1, Direction::East)];
            let y_edges = [(y == 0, Direction::South), (y == ny - 1, Direction::North)];
            for (_, dir) in x_edges.into_iter().chain(y_edges).filter(|(edge, _)| *edge) {
                r.get_intf(link(dir as usize)).unused_and_tieoff(0);
            }
        }
    }
    top
}

/// [`mesh`] of 2x2 routers, named `<name>_mesh_2x2`, with every local link
/// exported ([`Endpoint::Export`]) as `x<X>_y<Y>_p<P>_floo_<ch>_{i,o}`.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]), the package
/// cannot be written or the router does not import.
#[track_caller]
pub fn mesh_2x2(cfg: &FlooConfig, out_dir: &Path) -> ModDef {
    let mut spec = MeshSpec::new(2, 2);
    for (x, y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        spec.set(x, y, Endpoint::Export);
    }
    mesh(cfg, &spec, out_dir)
}
