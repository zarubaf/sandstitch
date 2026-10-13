// SPDX-License-Identifier: Apache-2.0

//! Helpers shared by the integration tests.

pub mod slang;

use topstitch::{Code, ModDef};

/// Asserts that `top.check()` reports exactly `expected`, in order: each
/// diagnostic's code and object (definition, end, msb, lsb).
#[track_caller]
pub fn assert_check(top: &ModDef, expected: &[(Code, &str, &str, usize, usize)]) {
    let diagnostics = top.check();
    let found: Vec<_> = diagnostics
        .0
        .iter()
        .map(|d| {
            let o = d
                .object
                .as_ref()
                .unwrap_or_else(|| panic!("no object: {}", diagnostics.render()));
            (d.code, o.definition.as_str(), o.end.as_str(), o.msb, o.lsb)
        })
        .collect();
    assert_eq!(found, expected, "{}", diagnostics.render());
}
