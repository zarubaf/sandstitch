// SPDX-License-Identifier: Apache-2.0

use topstitch::diagnostic::codes;
use topstitch::*;

use crate::common::assert_check;

#[test]
fn test_unused_bits_marked_correctly() {
    let mod_def = ModDef::new("TestMod");
    let in_port = mod_def.add_port("in", IO::Input(8));
    let out_port = mod_def.add_port("out", IO::Output(8));

    out_port.bit(0).connect(&in_port.bit(0));
    out_port.bit(7).connect(&in_port.bit(7));
    out_port.slice(6, 1).tieoff(0);

    in_port.slice(6, 1).unused();

    mod_def.emit(EmitOptions::default()); // Should pass
}

#[test]
fn test_unused_bits_not_marked() {
    let mod_def = ModDef::new("TestMod");
    let in_port = mod_def.add_port("in", IO::Input(8));
    let out_port = mod_def.add_port("out", IO::Output(8));

    out_port.bit(0).connect(&in_port.bit(0));
    out_port.bit(7).connect(&in_port.bit(7));
    out_port.slice(6, 1).tieoff(0);

    assert_check(&mod_def, &[(codes::S0402, "TestMod", "in", 6, 1)]);
}

#[test]
fn test_mod_def_input_fully_unused() {
    let mod_def = ModDef::new("TestMod");
    mod_def.add_port("in", IO::Input(8));
    assert_check(&mod_def, &[(codes::S0402, "TestMod", "in", 7, 0)]);
}

#[test]
fn test_mod_inst_output_fully_unused() {
    let leaf = ModDef::new("LeafMod");
    leaf.add_port("out", IO::Output(8)).tieoff(42);
    let top = ModDef::new("TopMod");
    top.instantiate(&leaf, Some("leaf_inst"), None);
    assert_check(&top, &[(codes::S0402, "TopMod", "leaf_inst.out", 7, 0)]);
}
