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
