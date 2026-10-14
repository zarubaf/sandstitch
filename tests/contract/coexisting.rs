// SPDX-License-Identifier: Apache-2.0

//! Definitions with distinct keys coexist in one design; equal definitions
//! with one key are emitted once and different ones are rejected.

use topstitch::*;

use crate::common::slang::Sources;

/// The leaf a generated adder instantiates.
const LEAF: &str = "\
module add #(parameter int WIDTH = 8) (
  input [WIDTH-1:0] a,
  input [WIDTH-1:0] b,
  output [WIDTH-1:0] y
);
endmodule
";

/// A definition named `name` that wraps an `add` of `width` bits.
fn adder(sources: &Sources, name: &str, width: usize) -> ModDef {
    let leaf = sources
        .import("add")
        .try_parameterize(&[("WIDTH", width)])
        .unwrap();
    let def = ModDef::new(name);
    for port in ["a", "b"] {
        def.add_port(port, IO::Input(width));
    }
    def.add_port("y", IO::Output(width));
    let inst = def.instantiate(&leaf, Some("u"), None);
    for port in ["a", "b", "y"] {
        inst.get_port(port).connect(&def.get_port(port));
    }
    def
}

/// Instantiates `def` in `top` as `inst`, wiring its ports to new top ports
/// named `<inst>_a`, `<inst>_b` and `<inst>_y`.
fn add_instance(top: &ModDef, def: &ModDef, inst: &str) {
    let width = def.get_port("a").io().width();
    let instance = top.instantiate(def, Some(inst), None);
    for (port, io) in [
        ("a", IO::Input(width)),
        ("b", IO::Input(width)),
        ("y", IO::Output(width)),
    ] {
        let top_port = top.add_port(format!("{inst}_{port}"), io);
        instance.get_port(port).connect(&top_port);
    }
}

#[test]
fn two_configurations_of_one_generator_elaborate() {
    let sources = Sources::new(&[LEAF]);
    let top = ModDef::new("top");
    add_instance(&top, &adder(&sources, "adder8", 8), "x0");
    add_instance(&top, &adder(&sources, "adder8", 8), "x1");
    add_instance(&top, &adder(&sources, "adder16", 16), "y");
    let diagnostics = top.check();
    assert!(diagnostics.0.is_empty(), "{}", diagnostics.render());

    let emitted = sources.emit_checked(&top);
    assert_eq!(emitted.matches("module adder8").count(), 1, "{emitted}");
    assert_eq!(emitted.matches("module adder16").count(), 1, "{emitted}");
    for inst in ["x0", "x1"] {
        assert!(
            emitted.contains(&format!("adder8 {inst}")),
            "{inst} is not an adder8:\n{emitted}"
        );
    }
}

#[test]
fn equal_generated_definitions_emit_once() {
    let sources = Sources::new(&[LEAF]);
    let top = ModDef::new("top");
    add_instance(&top, &adder(&sources, "adder8", 8), "x");
    add_instance(&top, &adder(&sources, "adder8", 8), "y");

    let emitted = sources.emit_checked(&top);
    assert_eq!(emitted.matches("module adder8").count(), 1, "{emitted}");
}

#[test]
#[should_panic(expected = "S0408")]
fn emit_panics_with_s0408() {
    let sources = Sources::new(&[LEAF]);
    let top = ModDef::new("top");
    add_instance(&top, &adder(&sources, "adder8", 8), "x");
    add_instance(&top, &adder(&sources, "adder8", 16), "y");
    top.emit(EmitOptions::default());
}
