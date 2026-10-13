// SPDX-License-Identifier: Apache-2.0

//! One driver feeding several sinks keeps each sink's bit range.

use topstitch::*;

use super::{LEAVES, expect, files};
use crate::common::slang::{Driver, Sources, check_map};

#[test]
fn one_driver_many_sinks() {
    let sources = Sources::new(&[LEAVES]);
    let (src8, snk8, snk4) = (
        sources.import("src8"),
        sources.import("snk8"),
        sources.import("snk4"),
    );
    let top = ModDef::new("t");
    top.add_port("y", IO::Output(8));
    let a = top.instantiate(&src8, Some("a"), None);
    let s1 = top.instantiate(&snk8, Some("s1"), None);
    let s2 = top.instantiate(&snk4, Some("s2"), None);
    s1.get_port("dst").connect(&a.get_port("o"));
    s2.get_port("d").connect(&a.get_port("o").slice(5, 2));
    top.get_port("y").connect(&a.get_port("o"));

    let emitted = sources.emit_checked(&top);
    let files = files(&sources);
    let map = [
        expect("s1.dst", 7, 0, Driver::End("a.o", 7, 0)),
        expect("s2.d", 3, 0, Driver::End("a.o", 5, 2)),
        expect("y", 7, 0, Driver::End("a.o", 7, 0)),
    ];
    let ends = [("a.o", 8), ("s1.dst", 8), ("s2.d", 4), ("y", 8)];
    check_map(&files, &emitted, "t", &ends, &map).unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}
