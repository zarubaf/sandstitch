// SPDX-License-Identifier: Apache-2.0

//! Snapshots of diagnostics produced by typed connections.

use super::{check_snapshot, panic_snapshot};
use topstitch::*;

const FIXTURE: &str = "tests/diagnostics/fixtures/typed.sv";

/// Imports `name` from the fixture.
fn import(name: &str) -> ModDef {
    let cfg = ParserConfig {
        sources: &[FIXTURE],
        ..Default::default()
    };
    ModDef::from_verilog_with_config(name, &cfg)
}

/// Slang reports absolute paths; make them relative to the crate root so
/// that snapshots do not depend on where the repository is checked out.
fn relative(mut diagnostics: Diagnostics) -> Diagnostics {
    let root = format!("{}/", env!("CARGO_MANIFEST_DIR"));
    for d in &mut diagnostics.0 {
        for label in std::iter::once(&mut d.primary).chain(&mut d.secondary) {
            if let Span::Sv { file, .. } = &mut label.span
                && let Some(rest) = file.strip_prefix(&root)
            {
                *file = rest.to_string();
            }
        }
    }
    diagnostics
}

#[test]
fn s0201_field_mismatch() {
    let producer = import("producer");
    let consumer = import("consumer");
    let top = ModDef::new("top");
    let p = top.instantiate(&producer, Some("p"), None);
    let c = top.instantiate(&consumer, Some("c"), None);
    p.get_port("req_o").connect(&c.get_port("req_i"));

    let diagnostics = relative(top.check());
    assert_eq!(diagnostics.0.len(), 1);
    assert_eq!(diagnostics.0[0].code, diagnostic::codes::S0201);
    check_snapshot("s0201_field_mismatch", &diagnostics);
}

#[test]
fn s0202_no_such_field() {
    let sel = import("sel");
    panic_snapshot("s0202_no_such_field", || {
        sel.get_port("hdrs_i").index(3).field("dst_idd");
    });
}

#[test]
fn s0203_index_out_of_range() {
    let sel = import("sel");
    panic_snapshot("s0203_index_out_of_range", || {
        sel.get_port("hdrs_i").index(8);
    });
}

#[test]
fn s0204_field_through_union() {
    let sel = import("sel");
    panic_snapshot("s0204_field_through_union", || {
        sel.get_port("u_i").field("hdr.dst_id");
    });
}
