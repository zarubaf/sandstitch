// SPDX-License-Identifier: Apache-2.0

//! A 2x2 mesh of `floo_nw_router` with XY routing.

use std::path::Path;

use topstitch::{IO, ModDef, ModInst};

use crate::{FlooConfig, link, router};

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

/// A mesh of four routers `i_router_x<X>_y<Y>` at coordinates (0..2, 0..2),
/// named `<name>_mesh_2x2`. Neighbours are linked East-West and
/// North-South; each router's `id_i` is tied to its coordinates (port ID
/// 0). Mesh-edge links are tied to zero (inputs) and marked unused
/// (outputs). The local links of each router (ports from
/// [`Direction::Eject`] on) are exported as `x<X>_y<Y>_p<P>_floo_<ch>_{i,o}`
/// for endpoints. Clock, reset and test enable are shared inputs. The
/// routing table input and the reduction offload interfaces are not used by
/// XY routing without collectives: inputs tied to zero, outputs unused.
///
/// The configuration package is written to `out_dir` ([`router`]).
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]), the package
/// cannot be written or the router does not import.
#[track_caller]
pub fn mesh_2x2(cfg: &FlooConfig, out_dir: &Path) -> ModDef {
    let router = router(cfg, out_dir).unwrap_or_else(|e| panic!("{e}"));
    let top = ModDef::new(format!("{}_mesh_2x2", cfg.name));
    let clk = top.add_port("clk_i", IO::Input(1));
    let rst = top.add_port("rst_ni", IO::Input(1));
    let test = top.add_port("test_enable_i", IO::Input(1));

    let mut routers = Vec::new();
    for x in 0..2usize {
        let mut column = Vec::new();
        for y in 0..2usize {
            let r = top.instantiate(&router, Some(&format!("i_router_x{x}_y{y}")), None);
            clk.connect(&r.get_port("clk_i"));
            rst.connect(&r.get_port("rst_ni"));
            test.connect(&r.get_port("test_enable_i"));
            let id = r.get_port("id_i");
            id.field("x").tieoff(x);
            id.field("y").tieoff(y);
            id.field("port_id").tieoff(0);
            r.get_port("id_route_map_i").tieoff(0);
            for side in ["wide", "narrow"] {
                r.get_port(format!("offload_{side}_rsp_i")).tieoff(0);
                r.get_port(format!("offload_{side}_req_o")).unused();
            }
            for p in Direction::Eject as usize..cfg.num_routes as usize {
                let name = format!("x{x}_y{y}_p{p}");
                r.get_intf(link(p))
                    .export_with_prefix(&name, format!("{name}_floo_"));
            }
            column.push(r);
        }
        routers.push(column);
    }

    connect(
        &routers[0][0],
        Direction::East,
        &routers[1][0],
        Direction::West,
    );
    connect(
        &routers[0][1],
        Direction::East,
        &routers[1][1],
        Direction::West,
    );
    connect(
        &routers[0][0],
        Direction::North,
        &routers[0][1],
        Direction::South,
    );
    connect(
        &routers[1][0],
        Direction::North,
        &routers[1][1],
        Direction::South,
    );

    for (x, y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let r = &routers[x][y];
        let x_edge = if x == 0 {
            Direction::West
        } else {
            Direction::East
        };
        let y_edge = if y == 0 {
            Direction::South
        } else {
            Direction::North
        };
        for dir in [x_edge, y_edge] {
            r.get_intf(link(dir as usize)).unused_and_tieoff(0);
        }
    }
    top
}
