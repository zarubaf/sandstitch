// SPDX-License-Identifier: Apache-2.0

//! Direction-aware interfaces of typed members (`def_intf_members`):
//! members keep their typed selection, a name may appear once per
//! direction, and connect, export and tie-off pair members by driving
//! direction. Bundle operations are compared with the same design wired
//! by hand, which is emitted and elaborated with Slang.

use topstitch::*;

use crate::common::slang::Sources;

const PKG: &str = "
package bpkg;
  typedef struct packed { logic valid; logic [7:0] data; } req_t;
  typedef struct packed { logic ready; logic [3:0] code; } rsp_t;
  typedef struct packed { logic valid; logic [5:0] data; logic [1:0] x; } req2_t;
endpackage
";

const MODS: &str = "
module duplex import bpkg::*; (
  input  struct packed { req_t req; rsp_t rsp; } ch_i,
  output struct packed { req_t req; rsp_t rsp; } ch_o
);
endmodule
module pad import bpkg::*; (input req_t req_i, output req_t req_o, inout wire [3:0] pad_io);
endmodule
module node import bpkg::*; (
  input  req_t [1:0] req_i,
  output req_t [1:0] req_o,
  input  rsp_t [1:0] rsp_i,
  output rsp_t [1:0] rsp_o
);
endmodule
module other_node import bpkg::*; (
  input  req2_t [1:0] req_i,
  output req2_t [1:0] req_o,
  input  rsp_t  [1:0] rsp_i,
  output rsp_t  [1:0] rsp_o
);
endmodule
";

/// Defines `link<d>` on `m` for both elements `d`: `req` and `rsp` in both
/// directions.
fn def_links(m: &ModDef) {
    for d in 0..2 {
        m.def_intf_members(
            format!("link{d}"),
            [
                ("req", m.get_port("req_o").index(d)),
                ("req", m.get_port("req_i").index(d)),
                ("rsp", m.get_port("rsp_o").index(d)),
                ("rsp", m.get_port("rsp_i").index(d)),
            ],
        );
    }
}

fn node(sources: &Sources) -> ModDef {
    let node = sources.import("node");
    def_links(&node);
    node
}

/// Ties off and marks unused every element of `inst` not used by a test.
fn close(inst: &ModInst, d: usize) {
    for ch in ["req", "rsp"] {
        inst.get_port(format!("{ch}_i")).index(d).tieoff(0);
        inst.get_port(format!("{ch}_o")).index(d).unused();
    }
}

/// Links element `da` of `a` with element `db` of `b` by hand.
fn hand_link(a: &ModInst, da: usize, b: &ModInst, db: usize) {
    for ch in ["req", "rsp"] {
        a.get_port(format!("{ch}_o"))
            .index(da)
            .connect(&b.get_port(format!("{ch}_i")).index(db));
        b.get_port(format!("{ch}_o"))
            .index(db)
            .connect(&a.get_port(format!("{ch}_i")).index(da));
    }
}

#[test]
fn members_keep_their_selection() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let link = node.get_intf("link1");
    let names: Vec<String> = link.members().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["req", "req", "rsp", "rsp"]);
    for (_, slice) in link.members() {
        let element = node.get_port(slice.get_port().name()).index(1);
        assert!(element.port_type().is_some());
        assert_eq!(slice.port_type(), element.port_type());
        assert_eq!(slice, element);
    }

    // On an instance, members are the same elements of the instance ports,
    // not the whole array ports.
    let top = ModDef::new("top");
    let inst = top.instantiate(&node, Some("n"), None);
    for (_, slice) in inst.get_intf("link0").members() {
        let element = inst.get_port(slice.get_port().name()).index(0);
        assert_eq!(slice.port_type(), element.port_type(), "{slice:?}");
        assert_eq!(slice, element);
    }
}

#[test]
fn peer_connect_matches_hand_wiring() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);

    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let a = top.instantiate(&node, Some("a"), None);
        let b = top.instantiate(&node, Some("b"), None);
        if by_hand {
            hand_link(&a, 0, &b, 1);
        } else {
            a.get_intf("link0").connect(&b.get_intf("link1"), false);
        }
        close(&a, 1);
        close(&b, 0);
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn hand_wiring_comparison_is_not_vacuous() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let emit = |da: usize| {
        let top = ModDef::new("top");
        let a = top.instantiate(&node, Some("a"), None);
        let b = top.instantiate(&node, Some("b"), None);
        hand_link(&a, da, &b, 1);
        close(&a, 1 - da);
        close(&b, 0);
        sources.emit_checked(&top)
    };
    assert_ne!(emit(0), emit(1));
}

#[test]
fn parent_to_child_connect_keeps_directions() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let width = |p: &str| node.get_port(p).index(0).width();

    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let n = top.instantiate(&node, Some("n"), None);
        let ports = [
            ("req", top.add_port("up_req_i", IO::Input(width("req_i")))),
            ("req", top.add_port("up_req_o", IO::Output(width("req_o")))),
            ("rsp", top.add_port("up_rsp_i", IO::Input(width("rsp_i")))),
            ("rsp", top.add_port("up_rsp_o", IO::Output(width("rsp_o")))),
        ];
        if by_hand {
            for ch in ["req", "rsp"] {
                top.get_port(format!("up_{ch}_i"))
                    .connect(&n.get_port(format!("{ch}_i")).index(0));
                n.get_port(format!("{ch}_o"))
                    .index(0)
                    .connect(&top.get_port(format!("up_{ch}_o")));
            }
        } else {
            let up = top.def_intf_members("up", ports.map(|(n, p)| (n, p.to_port_slice())));
            up.connect(&n.get_intf("link0"), false);
        }
        close(&n, 1);
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn connecting_different_structs_is_s0201() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let other = sources.import("other_node");
    def_links(&other);

    let top = ModDef::new("top");
    let a = top.instantiate(&node, Some("a"), None);
    let b = top.instantiate(&other, Some("b"), None);
    a.get_intf("link0").connect(&b.get_intf("link0"), false);
    close(&a, 1);
    close(&b, 1);
    let codes: Vec<Code> = top.check().0.iter().map(|d| d.code).collect();
    assert_eq!(codes, [diagnostic::codes::S0201, diagnostic::codes::S0201]);
}

#[test]
fn unpartnered_members_are_skipped_with_allow_mismatch() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    node.def_intf_members(
        "req_only",
        [
            ("req", node.get_port("req_o").index(0)),
            ("req", node.get_port("req_i").index(0)),
        ],
    );

    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let a = top.instantiate(&node, Some("a"), None);
        let b = top.instantiate(&node, Some("b"), None);
        if by_hand {
            a.get_port("req_o")
                .index(0)
                .connect(&b.get_port("req_i").index(1));
            b.get_port("req_o")
                .index(1)
                .connect(&a.get_port("req_i").index(0));
        } else {
            a.get_intf("req_only").connect(&b.get_intf("link1"), true);
        }
        a.get_port("rsp_i").index(0).tieoff(0);
        a.get_port("rsp_o").index(0).unused();
        b.get_port("rsp_i").index(1).tieoff(0);
        b.get_port("rsp_o").index(1).unused();
        close(&a, 1);
        close(&b, 0);
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn export_names_both_directions() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let top = ModDef::new("top");
    let n = top.instantiate(&node, Some("n"), None);
    let up = n.get_intf("link0").export_with_prefix("up", "up_");
    close(&n, 1);

    let ports: Vec<(String, String, Option<PortType>)> = top
        .get_ports(None)
        .iter()
        .map(|p| (p.name().to_string(), format!("{:?}", p.io()), p.port_type()))
        .collect();
    let elem = |p: &str| node.get_port(p).index(0).port_type();
    let w = |p: &str| node.get_port(p).index(0).width();
    assert_eq!(
        ports,
        [
            (
                "up_req_o".into(),
                format!("{:?}", IO::Output(w("req_o"))),
                elem("req_o")
            ),
            (
                "up_req_i".into(),
                format!("{:?}", IO::Input(w("req_i"))),
                elem("req_i")
            ),
            (
                "up_rsp_o".into(),
                format!("{:?}", IO::Output(w("rsp_o"))),
                elem("rsp_o")
            ),
            (
                "up_rsp_i".into(),
                format!("{:?}", IO::Input(w("rsp_i"))),
                elem("rsp_i")
            ),
        ]
    );
    let members: Vec<(String, String)> = up
        .members()
        .into_iter()
        .map(|(n, s)| (n, s.get_port().name().to_string()))
        .collect();
    assert_eq!(
        members,
        [
            ("req".into(), "up_req_o".into()),
            ("req".into(), "up_req_i".into()),
            ("rsp".into(), "up_rsp_o".into()),
            ("rsp".into(), "up_rsp_i".into()),
        ]
    );
    sources.emit_checked(&top);
}

#[test]
fn unused_and_tieoff_closes_both_directions() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let n = top.instantiate(&node, Some("n"), None);
        if by_hand {
            close(&n, 0);
        } else {
            n.get_intf("link0").unused_and_tieoff(0);
        }
        close(&n, 1);
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn def_intf_resolves_ports_when_used() {
    // As upstream: a `(port, msb, lsb)` mapping may name a port added later.
    let m = ModDef::new("w");
    let mapping = indexmap::IndexMap::from([("a".to_string(), ("a_i".to_string(), 3, 0))]);
    let intf = m.def_intf("x", mapping);
    m.add_port("a_i", IO::Input(4));
    assert_eq!(intf.get("a").unwrap().width(), 4);
}

#[test]
fn export_of_module_interface_names_the_exported_direction() {
    // Exporting a port of the module itself flips its direction, and the
    // suffix names the exported port's direction.
    let m = ModDef::new("w");
    let a_i = m.add_port("a_i", IO::Input(9));
    let a_o = m.add_port("a_o", IO::Output(9));
    m.def_intf_members(
        "x",
        [("a", a_i.to_port_slice()), ("a", a_o.to_port_slice())],
    )
    .export_with_prefix("y", "e_");
    let io = |name: &str| format!("{:?}", m.get_port(name).io());
    assert_eq!(io("e_a_o"), format!("{:?}", IO::Output(9)));
    assert_eq!(io("e_a_i"), format!("{:?}", IO::Input(9)));
}

#[test]
fn field_members_connect_like_hand_wiring() {
    let sources = Sources::new(&[PKG, MODS]);
    let duplex = sources.import("duplex");
    let link = duplex.def_intf_members(
        "link",
        [
            ("req", duplex.get_port("ch_o").field("req")),
            ("req", duplex.get_port("ch_i").field("req")),
            ("rsp", duplex.get_port("ch_o").field("rsp")),
            ("rsp", duplex.get_port("ch_i").field("rsp")),
        ],
    );
    for (_, slice) in link.members() {
        let field = duplex
            .get_port(slice.get_port().name())
            .field(if slice.width() == 9 { "req" } else { "rsp" });
        assert_eq!(slice.port_type(), field.port_type());
        assert_eq!(slice, field);
    }

    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let a = top.instantiate(&duplex, Some("a"), None);
        let b = top.instantiate(&duplex, Some("b"), None);
        if by_hand {
            for f in ["req", "rsp"] {
                a.get_port("ch_o")
                    .field(f)
                    .connect(&b.get_port("ch_i").field(f));
                b.get_port("ch_o")
                    .field(f)
                    .connect(&a.get_port("ch_i").field(f));
            }
        } else {
            a.get_intf("link").connect(&b.get_intf("link"), false);
        }
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn export_without_repeated_names_adds_no_suffix() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    node.def_intf_members(
        "out",
        [
            ("req", node.get_port("req_o").index(0)),
            ("rsp", node.get_port("rsp_o").index(0)),
        ],
    );
    let top = ModDef::new("top");
    let n = top.instantiate(&node, Some("n"), None);
    n.get_intf("out").export_with_prefix("up", "up_");
    n.get_port("req_i").tieoff(0);
    n.get_port("rsp_i").tieoff(0);
    n.get_port("req_o").index(1).unused();
    n.get_port("rsp_o").index(1).unused();

    let ports: Vec<(String, Option<PortType>)> = top
        .get_ports(None)
        .iter()
        .map(|p| (p.name().to_string(), p.port_type()))
        .collect();
    assert_eq!(
        ports,
        [
            ("up_req".into(), node.get_port("req_o").index(0).port_type()),
            ("up_rsp".into(), node.get_port("rsp_o").index(0).port_type()),
        ]
    );
    sources.emit_checked(&top);
}

#[test]
fn typed_parent_ports_drive_child_inputs() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);

    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        // `n`'s link 1 exported gives the parent typed ports `up_*_i`.
        let n = top.instantiate(&node, Some("n"), None);
        let m = top.instantiate(&node, Some("m"), None);
        let up = n.get_intf("link1").export_with_prefix("up", "up_");
        if by_hand {
            for ch in ["req", "rsp"] {
                top.get_port(format!("up_{ch}_i"))
                    .connect(&m.get_port(format!("{ch}_i")).index(0));
            }
        } else {
            // The parent's inputs (driving its body) pair with the child's
            // inputs; the parent's outputs are already driven by `n`.
            let inputs = up
                .members()
                .into_iter()
                .filter(|(_, s)| matches!(s.get_port().io(), IO::Input(_)));
            let up_in = top.def_intf_members("up_in", inputs);
            up_in.connect(&m.get_intf("link0"), true);
        }
        for ch in ["req", "rsp"] {
            m.get_port(format!("{ch}_o")).index(0).unused();
        }
        close(&n, 0);
        close(&m, 1);
        sources.emit_checked(&top)
    };
    let emitted = build(false);
    assert_eq!(emitted, build(true));
    assert!(emitted.contains("input bpkg::req_t up_req_i"), "{emitted}");
}

#[test]
fn tieoff_and_unused_alone_touch_one_direction() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let n = top.instantiate(&node, Some("n"), None);
        if by_hand {
            for ch in ["req", "rsp"] {
                n.get_port(format!("{ch}_i")).index(0).tieoff(0);
            }
        } else {
            n.get_intf("link0").tieoff(0);
        }
        // Outputs are left alone by `tieoff`; exporting them shows they are
        // still free.
        for ch in ["req", "rsp"] {
            n.get_port(format!("{ch}_o"))
                .index(0)
                .export_as(format!("{ch}0"));
        }
        if by_hand {
            for ch in ["req", "rsp"] {
                n.get_port(format!("{ch}_o")).index(1).unused();
            }
        } else {
            n.get_intf("link1").unused();
        }
        for ch in ["req", "rsp"] {
            n.get_port(format!("{ch}_i")).index(1).tieoff(0);
        }
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn export_without_prefix_keeps_both_directions() {
    let sources = Sources::new(&[PKG, MODS]);
    let node = node(&sources);
    let top = ModDef::new("top");
    let n = top.instantiate(&node, Some("n"), None);
    let up = n.get_intf("link0").export();
    close(&n, 1);
    let members: Vec<(String, String, String)> = up
        .members()
        .into_iter()
        .map(|(m, s)| {
            (
                m,
                s.get_port().name().to_string(),
                format!("{:?}", s.get_port().io()),
            )
        })
        .collect();
    let w = |p: &str| node.get_port(p).index(0).width();
    assert_eq!(
        members,
        [
            (
                "req".into(),
                "req_o".into(),
                format!("{:?}", IO::Output(w("req_o")))
            ),
            (
                "req".into(),
                "req_i".into(),
                format!("{:?}", IO::Input(w("req_i")))
            ),
            (
                "rsp".into(),
                "rsp_o".into(),
                format!("{:?}", IO::Output(w("rsp_o")))
            ),
            (
                "rsp".into(),
                "rsp_i".into(),
                format!("{:?}", IO::Input(w("rsp_i")))
            ),
        ]
    );
    sources.emit_checked(&top);
}

#[test]
fn inout_members_pair_with_inouts() {
    // `req` in both directions makes `connect` pair by direction, so the
    // inout pairs only because inouts pair with inouts.
    let sources = Sources::new(&[PKG, MODS]);
    let pad = sources.import("pad");
    pad.def_intf_members(
        "bus",
        [
            ("req", pad.get_port("req_o").to_port_slice()),
            ("req", pad.get_port("req_i").to_port_slice()),
            ("pad", pad.get_port("pad_io").to_port_slice()),
        ],
    );
    let build = |by_hand: bool| {
        let top = ModDef::new("top");
        let a = top.instantiate(&pad, Some("a"), None);
        let b = top.instantiate(&pad, Some("b"), None);
        if by_hand {
            a.get_port("req_o").connect(&b.get_port("req_i"));
            b.get_port("req_o").connect(&a.get_port("req_i"));
            a.get_port("pad_io").connect(&b.get_port("pad_io"));
        } else {
            a.get_intf("bus").connect(&b.get_intf("bus"), false);
        }
        sources.emit_checked(&top)
    };
    assert_eq!(build(false), build(true));
}
