// SPDX-License-Identifier: Apache-2.0

//! Bits marked unused are not missing; bits that are neither connected, tied
//! off nor marked unused are.

use topstitch::diagnostic::codes;
use topstitch::*;

use super::{LEAVES, expect, files};
use crate::common::assert_check;
use crate::common::slang::{Driver, Sources, check_map};

#[test]
fn open_and_unused_are_not_missing() {
    let sources = Sources::new(&[LEAVES]);
    let (src8, snk8) = (sources.import("src8"), sources.import("snk8"));
    let top = ModDef::new("t");
    let a = top.instantiate(&src8, Some("a"), None);
    let u = top.instantiate(&snk8, Some("u"), None);
    a.get_port("o").slice(7, 4).unused();
    u.get_port("dst")
        .slice(7, 4)
        .connect(&a.get_port("o").slice(3, 0));
    assert_check(&top, &[(codes::S0402, "t", "u.dst", 3, 0)]);

    u.get_port("dst").slice(3, 0).tieoff(0);
    assert_check(&top, &[]);
    let emitted = sources.emit_checked(&top);
    let files = files(&sources);
    let map = [
        expect("a.o", 7, 4, Driver::Unused),
        expect("u.dst", 7, 4, Driver::End("a.o", 3, 0)),
        expect("u.dst", 3, 0, Driver::Const(0)),
    ];
    let ends = [("a.o", 8), ("u.dst", 8)];
    check_map(&files, &emitted, "t", &ends, &map).unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}
