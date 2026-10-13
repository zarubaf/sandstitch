// SPDX-License-Identifier: Apache-2.0

//! Slices, fan-out and exposed nested ports across a generated hierarchy.

use std::path::Path;

use topstitch::*;

use super::LEAVES;
use crate::common::slang::{Driver, Expect, Sources, check_map};

#[test]
fn exposed_nested_ports() {
    let sources = Sources::new(&[LEAVES]);
    let (src8, snk8, snk4) = (
        sources.import("src8"),
        sources.import("snk8"),
        sources.import("snk4"),
    );

    let mid = ModDef::new("mid");
    let s = mid.instantiate(&src8, Some("s"), None);
    let k = mid.instantiate(&snk8, Some("k"), None);
    s.get_port("o").export_as("o");
    k.get_port("dst").export_as("i");

    let top = ModDef::new("t");
    let m = top.instantiate(&mid, Some("m"), None);
    let p = top.instantiate(&snk4, Some("p"), None);
    let p2 = top.instantiate(&snk4, Some("p2"), None);
    let q = top.instantiate(&src8, Some("q"), None);
    p.get_port("d").connect(&m.get_port("o").slice(6, 3));
    p2.get_port("d").connect(&m.get_port("o").slice(6, 3));
    m.get_port("i")
        .slice(7, 4)
        .connect(&q.get_port("o").slice(3, 0));
    m.get_port("i")
        .slice(3, 0)
        .connect(&q.get_port("o").slice(7, 4));
    m.get_port("o").slice(7, 7).unused();
    m.get_port("o").slice(2, 0).unused();

    let emitted = sources.emit_checked(&top);
    let files: Vec<&Path> = sources.files.iter().map(|f| f.path()).collect();
    let e = |end, msb, lsb, driver| Expect {
        end,
        msb,
        lsb,
        driver,
    };

    let mid_map = [
        e("o", 7, 0, Driver::End("s.o", 7, 0)),
        e("k.dst", 7, 0, Driver::End("i", 7, 0)),
    ];
    let mid_ends = [("s.o", 8), ("k.dst", 8), ("o", 8), ("i", 8)];
    check_map(&files, &emitted, "mid", &mid_ends, &mid_map)
        .unwrap_or_else(|e| panic!("{emitted}\n{e}"));

    let top_map = [
        e("p.d", 3, 0, Driver::End("m.o", 6, 3)),
        e("p2.d", 3, 0, Driver::End("m.o", 6, 3)),
        e("m.i", 7, 4, Driver::End("q.o", 3, 0)),
        e("m.i", 3, 0, Driver::End("q.o", 7, 4)),
        e("m.o", 7, 7, Driver::Unused),
        e("m.o", 2, 0, Driver::Unused),
    ];
    let top_ends = [("p.d", 4), ("p2.d", 4), ("m.i", 8), ("m.o", 8), ("q.o", 8)];
    check_map(&files, &emitted, "t", &top_ends, &top_map)
        .unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}
