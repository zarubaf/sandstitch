// SPDX-License-Identifier: Apache-2.0

//! Tie-offs keep their value, width and bit range.

use topstitch::*;

use super::{LEAVES, expect, files};
use crate::common::slang::{Driver, Sources, check_map};

#[test]
fn tieoffs_keep_value_width_and_range() {
    let sources = Sources::new(&[LEAVES]);
    let (src8, snk8) = (sources.import("src8"), sources.import("snk8"));
    let top = ModDef::new("t");
    top.add_port("z", IO::Output(4));
    let a = top.instantiate(&src8, Some("a"), None);
    let u = top.instantiate(&snk8, Some("u"), None);
    u.get_port("dst").slice(7, 4).tieoff(0x5);
    u.get_port("dst")
        .slice(3, 0)
        .connect(&a.get_port("o").slice(3, 0));
    a.get_port("o").slice(7, 4).unused();
    top.get_port("z").tieoff(0xA);

    let emitted = sources.emit_checked(&top);
    let files = files(&sources);
    let map = [
        expect("u.dst", 7, 4, Driver::Const(0x5)),
        expect("u.dst", 3, 0, Driver::End("a.o", 3, 0)),
        expect("a.o", 7, 4, Driver::Unused),
        expect("z", 3, 0, Driver::Const(0xA)),
    ];
    let ends = [("a.o", 8), ("u.dst", 8), ("z", 4)];
    check_map(&files, &emitted, "t", &ends, &map).unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}
