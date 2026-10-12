// SPDX-License-Identifier: Apache-2.0

//! Round trip of typed emission through Slang: a design built from typed
//! imports is emitted, elaborated by Slang together with the sources its
//! manifest lists, and the nets Slang elaborates are compared with a
//! hand-written map of the expected types and widths.

use std::path::{Path, PathBuf};

use topstitch::*;

use crate::common::slang::{Connection, Sources, check_connections, elaborate, top_nets};

const PKG: &str = "
package rt_pkg;
  typedef struct packed { logic valid; logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { hdr_t hdr; logic [31:0] data; } flit_t;
  typedef struct packed { logic [7:0] len; logic [3:0] dst_id; logic valid; } hdr_rev_t;
endpackage
";

const CHILD: &str = "
module node #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output T out_o
);
endmodule
";

const SINKS: &str = "
module flit_sink (input rt_pkg::flit_t f);
endmodule
module bit_sink (input logic v);
endmodule
module rev_sink (input rt_pkg::hdr_rev_t r);
endmodule
";

fn ty(value: &str) -> ParameterValue {
    ParameterValue::Type(value.to_string())
}

#[test]
fn typed_design_round_trips_through_slang() {
    let pkg = Sources::new(&[PKG]);
    let child = Sources::new(&[CHILD]);
    let sinks = Sources::new(&[SINKS]);
    let import = |paths: &[&str], name: &str| {
        let cfg = ParserConfig {
            sources: paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    };
    let (p, c, s) = (pkg.paths()[0], child.paths()[0], sinks.paths()[0]);
    let node = import(&[p, c], "node");
    let (flit_sink, bit_sink, rev_sink) = (
        import(&[p, s], "flit_sink"),
        import(&[p, s], "bit_sink"),
        import(&[p, s], "rev_sink"),
    );
    // Two type specializations of one module.
    let flit_node = node.parameterize(&[("T", ty("rt_pkg::flit_t"))]);
    let hdr_node = node.parameterize(&[("T", ty("rt_pkg::hdr_t"))]);

    let top = ModDef::new("rt_top");
    let a = top.instantiate(&flit_node, Some("a"), None);
    let b = top.instantiate(&hdr_node, Some("b"), None);
    let f0 = top.instantiate(&flit_sink, Some("f0"), None);
    let f1 = top.instantiate(&flit_sink, Some("f1"), None);
    let v = top.instantiate(&bit_sink, Some("v"), None);
    let r = top.instantiate(&rev_sink, Some("r"), None);

    let a_in = a.get_port("in_i").export_as("a_in");
    // Fan-out: one source, two sinks.
    a.get_port("out_o").connect(&f0.get_port("f"));
    a.get_port("out_o").connect(&f1.get_port("f"));
    // A field connection into an element.
    a.get_port("out_o")
        .field("hdr")
        .connect(&b.get_port("in_i").index(0));
    // An index connection from a typed parent port.
    a_in.index(1).field("hdr.valid").connect(&v.get_port("v"));
    b.get_port("in_i").index(1).export_as("h_in");
    // A bit-level reinterpretation as another layout: the net goes flat.
    // Exporting its `valid` field (bit 12) hits a known emitter defect: the
    // exported bit stays declared in `b_out_o` but unused (listed as
    // remaining work for #4).
    b.get_port("out_o").connect_bits(&r.get_port("r"));
    b.get_port("out_o").field("valid").export_as("b_valid");

    let emitted = top.emit(EmitOptions::default());
    assert_eq!(
        emitted,
        "\
module rt_top(
  input rt_pkg::flit_t [1:0] a_in,
  input rt_pkg::hdr_t h_in,
  output wire b_valid
);
  wire rt_pkg::flit_t a_out_o;
  node #(
    .T(rt_pkg::flit_t)
  ) a (
    .in_i(a_in),
    .out_o(a_out_o)
  );
  wire [12:0] b_out_o;
  node #(
    .T(rt_pkg::hdr_t)
  ) b (
    .in_i({h_in, a_out_o.hdr}),
    .out_o({b_valid, b_out_o[11:0]})
  );
  flit_sink f0 (
    .f(a_out_o)
  );
  flit_sink f1 (
    .f(a_out_o)
  );
  bit_sink v (
    .v(a_in[1].hdr.valid)
  );
  rev_sink r (
    .r({b_valid, b_out_o[11:0]})
  );
endmodule
"
    );

    let manifest = top.manifest();
    let abs = |s: &str| std::path::absolute(s).unwrap();
    assert_eq!(manifest.sources, vec![abs(p), abs(c), abs(s)]);
    let files: Vec<&Path> = manifest.sources.iter().map(PathBuf::as_path).collect();
    elaborate(&files, &emitted, "rt_top").unwrap_or_else(|e| panic!("{emitted}\n{e}"));

    // By hand: `hdr_t` is 1 + 4 + 8 = 13 bits, `flit_t` 13 + 32 = 45.
    let expected: Vec<(&str, (&str, usize))> = vec![
        ("a_in", ("rt_pkg::flit_t[1:0]", 90)),
        ("a_out_o", ("rt_pkg::flit_t", 45)),
        ("b_out_o", ("logic[12:0]", 13)),
        ("b_valid", ("logic", 1)),
        ("h_in", ("rt_pkg::hdr_t", 13)),
    ];
    let nets = top_nets(&files, &emitted, "rt_top").unwrap_or_else(|e| panic!("{e}"));
    let nets: Vec<(&str, (&str, usize))> = nets
        .iter()
        .map(|(name, (ty, width))| (name.as_str(), (ty.as_str(), *width)))
        .collect();
    assert_eq!(nets, expected);

    // Every emitted connection moves the intended bits, evaluated by Slang
    // on the emitted expressions. By hand: `flit_t` is hdr [44:32] (valid
    // 44) and data [31:0]; element 1 of `a_in` is [89:45]; `in_i` of `b`
    // is element 1 [25:13] and element 0 [12:0]; `hdr_t.valid` is bit 12.
    let connections = connections();
    check_connections(&files, &emitted, &expected, &connections).unwrap_or_else(|e| panic!("{e}"));

    // The check is not vacuous: swapped elements, a wrong element, and a
    // reversed reinterpretation each fail it.
    for (from, to, culprit) in [
        (
            ".in_i({h_in, a_out_o.hdr})",
            ".in_i({a_out_o.hdr, h_in})",
            "b.in_i segment 0",
        ),
        (
            ".v(a_in[1].hdr.valid)",
            ".v(a_in[0].hdr.valid)",
            "v.v segment 0",
        ),
        (
            ".r({b_valid, b_out_o[11:0]})",
            ".r({b_out_o[11:0], b_valid})",
            "r.r segment 0",
        ),
    ] {
        assert!(emitted.contains(from));
        let mutated = emitted.replace(from, to);
        let err = check_connections(&files, &mutated, &expected, &connections).unwrap_err();
        // An elaboration-time assertion of the culprit, not a compile error.
        let assertion = format!("error: $error encountered: {culprit}: bit 0");
        assert!(err.contains(&assertion), "{to}: {err}");
        let errors = err.lines().filter(|l| l.contains(": error: ")).count();
        let assertions = err.matches("error: $error encountered: ").count();
        assert_eq!(errors, assertions, "{to}: {err}");
    }

    // `connect_bits` copies bits, not fields: `hdr_t.valid`, the bit
    // `b_valid` carries, is the top bit of `hdr_rev_t.len` at `r`.
    pkg.assert_bits("rt_pkg::hdr_t", &[(".valid", 12, 12)]);
    pkg.assert_bits("rt_pkg::hdr_rev_t", &[(".len", 12, 5)]);
}

/// The connections of `rt_top`, written by hand from the layouts.
fn connections() -> Vec<Connection<'static>> {
    let input = |inst, port, width, segments| Connection {
        inst,
        port,
        output: false,
        width,
        segments,
    };
    let output = |inst, port, width, segments| Connection {
        inst,
        port,
        output: true,
        width,
        segments,
    };
    vec![
        input("a", "in_i", 90, &[(89, 0, "a_in", 89, 0)]),
        output("a", "out_o", 45, &[(44, 0, "a_out_o", 44, 0)]),
        // A field into element 0, the typed parent port into element 1.
        input(
            "b",
            "in_i",
            26,
            &[(12, 0, "a_out_o", 44, 32), (25, 13, "h_in", 12, 0)],
        ),
        output(
            "b",
            "out_o",
            13,
            &[(12, 12, "b_valid", 0, 0), (11, 0, "b_out_o", 11, 0)],
        ),
        // Fan-out.
        input("f0", "f", 45, &[(44, 0, "a_out_o", 44, 0)]),
        input("f1", "f", 45, &[(44, 0, "a_out_o", 44, 0)]),
        // An element's field: `a_in[1].hdr.valid`.
        input("v", "v", 1, &[(0, 0, "a_in", 89, 89)]),
        // `connect_bits`: `b`'s output bits, unchanged.
        input(
            "r",
            "r",
            13,
            &[(12, 12, "b_valid", 0, 0), (11, 0, "b_out_o", 11, 0)],
        ),
    ]
}

#[test]
fn top_nets_reports_slang_types_and_widths() {
    // Hand-written RTL, so the helper is checked independently of emission.
    let pkg = Sources::new(&[PKG]);
    let rtl = "\
module chk(input rt_pkg::hdr_rev_t [2:0] p, output logic signed [4:0] q);
  wire rt_pkg::flit_t [0:1] x;
  wire [6:3] y;
  assign q = '0;
endmodule
";
    let files: Vec<&Path> = pkg.files.iter().map(|f| f.path()).collect();
    let nets = top_nets(&files, rtl, "chk").unwrap_or_else(|e| panic!("{e}"));
    let nets: Vec<(&str, (&str, usize))> = nets
        .iter()
        .map(|(name, (ty, width))| (name.as_str(), (ty.as_str(), *width)))
        .collect();
    assert_eq!(
        nets,
        vec![
            ("p", ("rt_pkg::hdr_rev_t[2:0]", 39)),
            ("q", ("logic signed[4:0]", 5)),
            ("x", ("rt_pkg::flit_t[0:1]", 90)),
            ("y", ("logic[6:3]", 4)),
        ]
    );
    // Slang errors are returned.
    let err = top_nets(
        &files,
        "module chk; wire rt_pkg::nope_t z; endmodule\n",
        "chk",
    )
    .unwrap_err();
    assert!(err.contains("nope_t"), "{err}");
}
