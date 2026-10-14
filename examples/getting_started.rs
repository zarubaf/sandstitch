// SPDX-License-Identifier: Apache-2.0

//! The walk-through of doc/guide.md: import a module, instantiate it twice,
//! connect, export ports, check, emit and list the manifest. Run it with
//! `cargo run --example getting_started`; `tests/examples.rs` runs it in CI.

use std::path::{Path, PathBuf};
use topstitch::{EmitOptions, ModDef};

/// Build the design, write its RTL to `out_dir/top.sv` and return the RTL.
pub fn run(out_dir: &Path) -> String {
    // Import `adder` from a SystemVerilog file. The first `false` is
    // `ignore_unknown_modules`: the file has no submodules to be unknown.
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/input/adder.sv");
    let adder = ModDef::from_verilog_file("adder", &file, false, false);
    let adder_9_bit = adder.parameterize(&[("W", 9)]);

    // Instantiate two adders: the second adds the 9-bit sum of the first to
    // a third input.
    let top = ModDef::new("top");
    let first = top.instantiate(&adder, Some("first"), None);
    let second = top.instantiate(&adder_9_bit, Some("second"), None);

    // Export the ports that face the outside.
    first.get_port("a").export_as("x");
    first.get_port("b").export_as("y");
    second.get_port("b").export_as("z");
    second.get_port("sum").export_as("sum");

    // Connect the instances.
    first.get_port("sum").connect(&second.get_port("a"));

    // Report problems as diagnostics, before emission.
    let diagnostics = top.check();
    assert!(!diagnostics.has_errors(), "{}", diagnostics.render());

    // Emit the RTL and list what it needs to build.
    let rtl = top.emit(EmitOptions::default());
    std::fs::create_dir_all(out_dir).expect("create the output directory");
    std::fs::write(out_dir.join("top.sv"), &rtl).expect("write top.sv");
    for source in top.manifest().sources {
        println!("source: {}", source.display());
    }
    rtl
}

#[allow(dead_code)]
fn main() {
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/output");
    println!("{}", run(&out_dir));
}
