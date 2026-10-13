// SPDX-License-Identifier: Apache-2.0

//! Concatenations keep their bit order in the emitted RTL.

use std::path::Path;

use topstitch::*;

use super::LEAVES;
use crate::common::slang::{Driver, Expect, Sources, check_map};

fn design(sources: &Sources) -> ModDef {
    let (src8, snk8) = (sources.import("src8"), sources.import("snk8"));
    let top = ModDef::new("t");
    let a = top.instantiate(&src8, Some("a"), None);
    let b = top.instantiate(&src8, Some("b"), None);
    let u = top.instantiate(&snk8, Some("u"), None);
    u.get_port("dst")
        .slice(7, 4)
        .connect(&a.get_port("o").slice(3, 0));
    u.get_port("dst")
        .slice(3, 0)
        .connect(&b.get_port("o").slice(7, 4));
    a.get_port("o").slice(7, 4).unused();
    b.get_port("o").slice(3, 0).unused();
    top
}

const ENDS: [(&str, usize); 3] = [("a.o", 8), ("b.o", 8), ("u.dst", 8)];

fn map() -> Vec<Expect<'static>> {
    let e = |end, msb, lsb, driver| Expect {
        end,
        msb,
        lsb,
        driver,
    };
    vec![
        e("u.dst", 7, 4, Driver::End("a.o", 3, 0)),
        e("u.dst", 3, 0, Driver::End("b.o", 7, 4)),
        e("a.o", 7, 4, Driver::Unused),
        e("b.o", 3, 0, Driver::Unused),
    ]
}

fn check(sources: &Sources, emitted: &str) -> Result<(), String> {
    let files: Vec<&Path> = sources.files.iter().map(|f| f.path()).collect();
    check_map(&files, emitted, "t", &ENDS, &map())
}

#[test]
fn concatenation_keeps_bit_order() {
    let sources = Sources::new(&[LEAVES]);
    let emitted = sources.emit_checked(&design(&sources));
    check(&sources, &emitted).unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}

#[test]
fn swapped_concatenation_is_caught() {
    let sources = Sources::new(&[LEAVES]);
    let emitted = sources.emit_checked(&design(&sources));
    let operands = ".dst({a_o[3:0], b_o[7:4]})";
    assert!(emitted.contains(operands), "{emitted}");
    let swapped = emitted.replace(operands, ".dst({b_o[7:4], a_o[3:0]})");
    let err = check(&sources, &swapped).unwrap_err();
    assert!(err.contains("u.dst"), "{err}");
}
