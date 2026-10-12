// SPDX-License-Identifier: Apache-2.0

//! A 2x2 mesh of `floo_nw_router` with XY routing.

use topstitch::{IO, ModDef, ModInst};

use crate::{FlooConfig, router};

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

const CHANNELS: [&str; 3] = ["req", "rsp", "wide"];

/// Connects the `dir` link of `a` to the `opposite` link of `b`, in both
/// directions: `a.floo_<ch>_o[dir]` drives `b.floo_<ch>_i[opposite]` and
/// `b.floo_<ch>_o[opposite]` drives `a.floo_<ch>_i[dir]`, for the request,
/// response and wide channels. Each link struct carries its own ready.
fn link(a: &ModInst, dir: Direction, b: &ModInst, opposite: Direction) {
    for ch in CHANNELS {
        let (o, i) = (format!("floo_{ch}_o"), format!("floo_{ch}_i"));
        a.get_port(&o)
            .index(dir as usize)
            .connect(&b.get_port(&i).index(opposite as usize));
        b.get_port(&o)
            .index(opposite as usize)
            .connect(&a.get_port(&i).index(dir as usize));
    }
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
/// Panics if the router does not import, or if `cfg` has fewer than five
/// routes or coordinates narrower than one bit.
#[track_caller]
pub fn mesh_2x2(cfg: &FlooConfig) -> ModDef {
    assert!(
        cfg.num_routes > Direction::Eject as u32,
        "{}: a mesh router needs at least {} routes, got {}",
        cfg.name,
        Direction::Eject as u32 + 1,
        cfg.num_routes
    );
    let router = router(cfg).unwrap_or_else(|e| panic!("{e}"));
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
                for ch in CHANNELS {
                    for dir in ["i", "o"] {
                        r.get_port(format!("floo_{ch}_{dir}"))
                            .index(p)
                            .export_as(format!("x{x}_y{y}_p{p}_floo_{ch}_{dir}"));
                    }
                }
            }
            column.push(r);
        }
        routers.push(column);
    }

    link(
        &routers[0][0],
        Direction::East,
        &routers[1][0],
        Direction::West,
    );
    link(
        &routers[0][1],
        Direction::East,
        &routers[1][1],
        Direction::West,
    );
    link(
        &routers[0][0],
        Direction::North,
        &routers[0][1],
        Direction::South,
    );
    link(
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
        for (dir, ch) in [x_edge, y_edge]
            .into_iter()
            .flat_map(|dir| CHANNELS.map(|ch| (dir, ch)))
        {
            r.get_port(format!("floo_{ch}_i"))
                .index(dir as usize)
                .tieoff(0);
            r.get_port(format!("floo_{ch}_o"))
                .index(dir as usize)
                .unused();
        }
    }
    top
}
