// SPDX-License-Identifier: Apache-2.0

//! Snapshots of diagnostics produced by direction-aware interfaces.

use super::panic_snapshot;
use topstitch::*;

const FIXTURE: &str = "tests/diagnostics/fixtures/intf.sv";

fn node() -> ModDef {
    let cfg = ParserConfig {
        sources: &[FIXTURE],
        ..Default::default()
    };
    ModDef::from_verilog_with_config("node", &cfg)
}

/// Defines `link` on `node`: `req` and `rsp` in both directions, element 0.
fn link(node: &ModDef) -> Intf {
    node.def_intf_members(
        "link",
        [
            ("req", node.get_port("req_o").index(0)),
            ("req", node.get_port("req_i").index(0)),
            ("rsp", node.get_port("rsp_o").index(0)),
            ("rsp", node.get_port("rsp_i").index(0)),
        ],
    )
}

#[test]
fn s0207_member_without_partner() {
    let node = node();
    link(&node);
    node.def_intf_members(
        "half",
        [
            ("req", node.get_port("req_o").index(1)),
            ("req", node.get_port("req_i").index(1)),
            ("rsp", node.get_port("rsp_o").index(1)),
        ],
    );
    let top = ModDef::new("top");
    let a = top.instantiate(&node, Some("a"), None);
    let b = top.instantiate(&node, Some("b"), None);
    panic_snapshot("s0207_member_without_partner", || {
        a.get_intf("link").connect(&b.get_intf("half"), false);
    });
}

#[test]
fn s0207_partner_missing_on_the_other_side() {
    let node = node();
    link(&node);
    node.def_intf_members(
        "half",
        [
            ("req", node.get_port("req_o").index(1)),
            ("req", node.get_port("req_i").index(1)),
            ("rsp", node.get_port("rsp_o").index(1)),
        ],
    );
    let top = ModDef::new("top");
    let a = top.instantiate(&node, Some("a"), None);
    let b = top.instantiate(&node, Some("b"), None);
    // `half` is the caller: its members all pair, `link`'s `rsp` output
    // does not.
    panic_snapshot("s0207_partner_missing_on_the_other_side", || {
        a.get_intf("half").connect(&b.get_intf("link"), false);
    });
}

#[test]
fn s0208_repeated_member() {
    let node = node();
    panic_snapshot("s0208_repeated_member", || {
        node.def_intf_members(
            "link",
            [
                ("req", node.get_port("req_o").index(0)),
                ("req", node.get_port("req_o").index(1)),
            ],
        );
    });
}

#[test]
fn s0209_get_by_name() {
    let node = node();
    let link = link(&node);
    panic_snapshot("s0209_get_by_name", || {
        link.get("req");
    });
}

#[test]
fn s0209_crossover() {
    let node = node();
    link(&node);
    let top = ModDef::new("top");
    let a = top.instantiate(&node, Some("a"), None);
    let b = top.instantiate(&node, Some("b"), None);
    panic_snapshot("s0209_crossover", || {
        a.get_intf("link")
            .crossover(&b.get_intf("link"), "(.*)", "(.*)");
    });
}

/// A named call that is expected to panic.
type Call<'a> = (&'static str, Box<dyn Fn() + 'a>);

/// Every operation that reads members by name reports S0209 at its caller.
#[test]
fn s0209_points_at_the_caller() {
    let node = node();
    let link = link(&node);
    let top = ModDef::new("top");
    let a = top.instantiate(&node, Some("a"), None);
    let b = top.instantiate(&node, Some("b"), None);
    let (la, lb) = (a.get_intf("link"), b.get_intf("link"));
    let calls: Vec<Call> = vec![
        (
            "set_max_distance",
            Box::new(|| la.set_max_distance(Some(3))),
        ),
        ("block_tracks", Box::new(|| drop(link.block_tracks()))),
        (
            "place_across_from",
            Box::new(|| la.place_across_from(&lb, false)),
        ),
        ("place_across", Box::new(|| la.place_across())),
        ("place_abutted", Box::new(|| la.place_abutted())),
        ("todo_jam_connect", Box::new(|| la.todo_jam_connect(&lb))),
        (
            "width",
            Box::new(|| {
                let _ = link.width();
            }),
        ),
        (
            "connect_intf",
            Box::new(|| {
                let port = |name: &str, io: IO| {
                    top.add_port(format!("{name}_{}", top.get_ports(None).len()), io)
                };
                Funnel::new(
                    (port("fa", IO::Input(8)), port("fa", IO::Output(8))),
                    (port("fb", IO::Input(8)), port("fb", IO::Output(8))),
                )
                .connect_intf(&la, &lb, false)
            }),
        ),
    ];
    for (what, call) in calls {
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).expect_err(what);
        let message = payload.downcast::<String>().expect("a String panic");
        assert!(message.contains("error[S0209]"), "{what}: {message}");
        assert!(
            message.contains("tests/diagnostics/intf.rs"),
            "{what}: {message}"
        );
    }
}
