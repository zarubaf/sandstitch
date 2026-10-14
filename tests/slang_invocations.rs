// SPDX-License-Identifier: Apache-2.0

//! The process-wide Slang run counter. This binary has one test, so no
//! other thread imports while it measures.

use topstitch::*;

#[test]
fn plain_import_runs_slang_once() {
    let before = slang_invocations();
    ModDef::from_verilog("A", "module A(input wire a); endmodule", true, false);
    assert_eq!(slang_invocations() - before, 1);
}
