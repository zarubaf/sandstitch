// SPDX-License-Identifier: Apache-2.0

//! Typed connections: field and index selections, type checks of
//! connections (S0201), `connect_bits`, and the selection errors
//! (S0202, S0203, S0204).

use indexmap::IndexMap;
use slang_rs::str2tmpfile;
use std::panic::{AssertUnwindSafe, catch_unwind};
use tempfile::NamedTempFile;
use topstitch::diagnostic::codes;
use topstitch::*;

const PKG: &str = "
package lay_pkg;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { hdr_t hdr; logic [31:0] data; } flit_t;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr2_t;
  typedef struct packed { hdr2_t hdr; logic [31:0] data; } flit2_t;
  typedef struct packed { logic [7:0] len; logic [3:0] dst_id; logic [31:0] data; } flat_t;
  typedef enum logic [1:0] { IDLE, BUSY, WAIT = 3 } state_t;
  typedef union packed { logic [11:0] raw; hdr_t hdr; } hdr_u;
  typedef struct packed {
    logic a;
    flit_t [2:0] flits;
    logic [0:3][1:0] asc;
    state_t [2:0] st;
  } wrap_t;
endpackage
";

const MODS: &str = "
module lay import lay_pkg::*; (input wrap_t [1:0] w, input hdr_u u, input logic [7:0] v);
endmodule
module src import lay_pkg::*; (output flit_t o, output hdr_t h, output flat_t f);
endmodule
module dst import lay_pkg::*; (input flit2_t i, input hdr2_t h, input flit_t i1);
endmodule
module ch (input logic [43:0] x_in, output logic [43:0] x_out);
endmodule
module pv_src import lay_pkg::*; (output logic [8:1] o, output wire s,
  output logic [3:0][1:0] q, output logic [0:7] r, output logic signed [7:0] g,
  output flit_t st, output state_t e);
endmodule
module pv_dst (input [7:0] i, input logic [0:0] s, input logic [7:0] q,
  input logic [7:0] r, input logic [7:0] g, input logic [43:0] st, input logic [1:0] e);
endmodule
";

/// Sources written to temp files; kept alive while the design is in use.
struct Sources {
    files: Vec<NamedTempFile>,
}

impl Sources {
    fn new(sources: &[&str]) -> Self {
        Sources {
            files: sources.iter().map(|s| str2tmpfile(s).unwrap()).collect(),
        }
    }

    fn try_import(&self, name: &str) -> Result<ModDef, ImportError> {
        let paths: Vec<&str> = self
            .files
            .iter()
            .map(|f| f.path().to_str().unwrap())
            .collect();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::try_from_verilog_with_config(name, &cfg)
    }

    fn import(&self, name: &str) -> ModDef {
        self.try_import(name).unwrap()
    }
}

fn range(slice: &PortSlice) -> (usize, usize) {
    (slice.msb(), slice.lsb())
}

fn logic(dims: &[(i64, i64)]) -> PortType {
    PortType::Logic {
        signed: false,
        packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn panic_message(f: impl FnOnce()) -> String {
    let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("expected a panic");
    match payload.downcast::<String>() {
        Ok(s) => *s,
        Err(payload) => payload.downcast_ref::<&str>().unwrap().to_string(),
    }
}

/// Every selection below, with its SV expression on the port `w` and the
/// bit range expected from the packed layout, computed by hand:
///
/// `wrap_t` is `a` (1) | `flits` (3 x 44) | `asc` (4 x 2) | `st` (3 x 2), 147
/// bits: `a` at 146, `flits` [145:14], `asc` [13:6], `st` [5:0]. Element 1 of
/// `w` adds 147. A `flit_t` is `hdr.dst_id` [43:40], `hdr.len` [39:32],
/// `data` [31:0]. `asc` is ascending, so `asc[0]` is its most significant
/// element.
fn selections(w: &Port) -> Vec<(&'static str, PortSlice, (usize, usize))> {
    vec![
        ("[0]", w.index(0), (146, 0)),
        ("[1]", w.index(1), (293, 147)),
        ("[1].a", w.index(1).field("a"), (293, 293)),
        (
            "[1].flits[2].hdr.len",
            w.index(1).field("flits").index(2).field("hdr.len"),
            (288, 281),
        ),
        (
            "[0].flits[2].hdr",
            w.index(0).field("flits").index(2).field("hdr"),
            (145, 134),
        ),
        (
            "[0].flits[0].data",
            w.index(0).field("flits").index(0).field("data"),
            (45, 14),
        ),
        ("[1].asc[0]", w.index(1).field("asc").index(0), (160, 159)),
        (
            "[1].asc[3][1]",
            w.index(1).field("asc").index(3).index(1),
            (154, 154),
        ),
        ("[0].st[1]", w.index(0).field("st").index(1), (3, 2)),
    ]
}

#[test]
fn field_and_index_ranges() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let w = lay.get_port("w");
    assert_eq!(w.io().width(), 294);
    for (expr, slice, expected) in selections(&w) {
        assert_eq!(range(&slice), expected, "w{expr}");
    }
}

#[test]
fn field_and_index_types() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let w = lay.get_port("w");
    let flits = w.index(1).field("flits");
    assert_eq!(
        flits.port_type().unwrap().to_string(),
        "lay_pkg::flit_t [2:0]"
    );
    assert_eq!(
        flits.index(2).port_type().unwrap().to_string(),
        "lay_pkg::flit_t"
    );
    assert_eq!(
        flits.index(2).field("hdr").port_type().unwrap().to_string(),
        "lay_pkg::hdr_t"
    );
    assert_eq!(
        flits.index(2).field("hdr.len").port_type(),
        Some(logic(&[(7, 0)]))
    );
    assert_eq!(
        w.index(1).field("asc").index(3).port_type(),
        Some(logic(&[(1, 0)]))
    );
    assert_eq!(
        w.index(1).field("asc").index(3).index(0).port_type(),
        Some(logic(&[]))
    );
    assert_eq!(
        w.index(0)
            .field("st")
            .index(1)
            .port_type()
            .unwrap()
            .to_string(),
        "lay_pkg::state_t"
    );
    // Chained `field` equals a dotted path.
    // (Slice equality compares bits only; the selection paths are compared
    // in the unit tests of `select.rs`.)
    let chained = flits.index(2).field("hdr").field("len");
    let dotted = flits.index(2).field("hdr.len");
    assert_eq!(range(&chained), (288, 281));
    assert_eq!(range(&dotted), (288, 281));
    assert_eq!(chained.port_type(), Some(logic(&[(7, 0)])));
    assert_eq!(dotted.port_type(), Some(logic(&[(7, 0)])));

    // The whole port is typed; plain bit slices are not, even full-width.
    assert_eq!(w.to_port_slice().port_type(), w.port_type());
    assert_eq!(w.slice(293, 0).port_type(), None);
    assert_eq!(w.bit(0).port_type(), None);
    assert_eq!(w.subdivide(2)[0].port_type(), None);

    // Selections on instance ports resolve against the instantiated module.
    let top = ModDef::new("top");
    let inst = top.instantiate(&lay, Some("u"), None);
    let on_inst = inst.get_port("w").index(1).field("flits").index(2);
    assert_eq!(range(&on_inst), (292, 249));
    assert_eq!(on_inst.port_type().unwrap().to_string(), "lay_pkg::flit_t");
}

/// Slang decides: for each selection, a value with only the expected LSB
/// (resp. MSB) set must read back as the lowest (resp. highest) bit of the
/// selected member.
fn layout_checker(w: &Port, wrong: bool) -> String {
    let width = w.io().width();
    let mut body = String::new();
    for (i, (expr, slice, _)) in selections(w).into_iter().enumerate() {
        let (msb, lsb) = range(&slice);
        let lsb = if wrong && i == 3 { lsb + 1 } else { lsb };
        let n = msb - lsb + 1;
        body += &format!(
            "  localparam arr_t L{i} = arr_t'({width}'(1) << {lsb});\n\
             \x20 if (L{i}{expr} != {n}'(1)) $error(\"lsb of w{expr}\");\n\
             \x20 localparam arr_t M{i} = arr_t'({width}'(1) << {msb});\n\
             \x20 if (M{i}{expr} != ({n}'(1) << {})) $error(\"msb of w{expr}\");\n",
            n - 1
        );
    }
    format!("module chk import lay_pkg::*; ();\n  typedef wrap_t [1:0] arr_t;\n{body}endmodule\n")
}

#[test]
fn ranges_agree_with_slang() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let w = lay.get_port("w");

    let checked = Sources::new(&[PKG, &layout_checker(&w, false)]);
    checked.try_import("chk").unwrap_or_else(|e| panic!("{e}"));

    // The checker is not vacuous: an off-by-one range is rejected.
    let wrong = Sources::new(&[PKG, &layout_checker(&w, true)]);
    let err = wrong
        .try_import("chk")
        .err()
        .expect("Slang rejects a wrong range");
    assert_eq!(err.diagnostics.0[0].code, codes::S0102);
    assert!(
        err.to_string().contains("lsb of w[1].flits[2].hdr.len"),
        "{err}"
    );
}

#[test]
fn equivalent_types_with_different_names_connect() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // flit_t -> flit2_t, and a field to a whole port: hdr_t -> hdr2_t.
    s.get_port("o").connect(&d.get_port("i"));
    s.get_port("o").field("hdr").connect(&d.get_port("h"));
    s.get_port("h").unused();
    s.get_port("f").unused();
    d.get_port("i1").tieoff(0);
    let diagnostics = top.check();
    assert_eq!(diagnostics, Diagnostics(vec![]));
    top.emit(EmitOptions::default());
}

#[test]
fn mismatch_is_reported_at_the_call_site() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // Same width (44), different layout.
    let line = line!() + 1;
    d.get_port("i1").connect(&s.get_port("f"));

    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    let diag = &diagnostics.0[0];
    assert_eq!(diag.code, codes::S0201);
    assert_eq!(
        diag.message,
        "incompatible types `lay_pkg::flit_t` and `lay_pkg::flat_t` in connection"
    );
    let Span::Rust { file, line: l, .. } = &diag.primary.span else {
        panic!("expected a Rust span: {:?}", diag.primary.span);
    };
    assert!(file.ends_with("tests/typed/connect.rs"), "{file}");
    assert_eq!(*l, line);
    assert_eq!(diag.secondary.len(), 2);
    assert_eq!(diag.secondary[0].span, d.get_port("i1").source_span());
    assert_eq!(diag.secondary[0].text, "`top.d.i1` is `lay_pkg::flit_t`");
    assert_eq!(diag.secondary[1].span, s.get_port("f").source_span());
    assert_eq!(diag.secondary[1].text, "`top.s.f` is `lay_pkg::flat_t`");
    assert_eq!(
        diag.notes,
        vec!["field `hdr`: `lay_pkg::hdr_t hdr` vs `logic [7:0] len`".to_string()]
    );
    assert_eq!(
        diag.helps,
        vec![
            "both ends must use equivalent types".to_string(),
            "to reinterpret bits deliberately, use `connect_bits`".to_string(),
        ]
    );
}

#[test]
fn nested_field_mismatch_names_the_field_path() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, lay) = (sources.import("src"), sources.import("lay"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let l = top.instantiate(&lay, Some("l"), None);
    // flat_t vs flit_t again, but through selections on both ends.
    l.get_port("w")
        .index(1)
        .field("flits")
        .index(0)
        .connect(&s.get_port("f"));
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 1);
    assert_eq!(
        diagnostics.0[0].secondary[0].text,
        "`top.l.w[1].flits[0]` is `lay_pkg::flit_t`"
    );
    assert_eq!(
        diagnostics.0[0].notes,
        vec!["field `hdr`: `lay_pkg::hdr_t hdr` vs `logic [7:0] len`".to_string()]
    );

    // A leaf mismatch reports the full path from the root of the types.
    let top = ModDef::new("top2");
    let s = top.instantiate(&src, Some("s"), None);
    let l = top.instantiate(&lay, Some("l"), None);
    l.get_port("w")
        .index(0)
        .field("flits")
        .index(1)
        .field("hdr")
        .connect(&s.get_port("o").field("hdr"));
    assert!(!top.check().has_errors());
    l.get_port("u").connect(&s.get_port("h"));
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 1);
    assert_eq!(
        diagnostics.0[0].notes,
        vec!["`lay_pkg::hdr_u` vs `lay_pkg::hdr_t`".to_string()]
    );
}

#[test]
fn connect_bits_skips_the_type_check_but_not_the_width_check() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    d.get_port("i1").connect_bits(&s.get_port("f"));
    d.get_port("h").slice(11, 0).connect_bits(&s.get_port("h"));
    assert_eq!(top.check(), Diagnostics(vec![]));

    let message = panic_message(|| d.get_port("i").connect_bits(&s.get_port("h")));
    assert!(message.starts_with("Width mismatch"), "{message}");
}

#[test]
fn plain_slices_are_checked_for_width_only() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // Full-width bit slices of differently laid out types.
    d.get_port("i1").slice(43, 0).connect(&s.get_port("f"));
    d.get_port("i").connect(&s.get_port("f").slice(43, 0));
    // An untyped port against a typed one.
    let plain = top.add_port("plain", IO::Input(12));
    d.get_port("h").connect(&plain);
    assert_eq!(top.check(), Diagnostics(vec![]));
}

#[test]
fn jam_connect_streams_bits_without_type_check() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // Whole typed ports of equal width but different layouts.
    d.get_port("i1").todo_jam_connect(&s.get_port("f"));
    assert_eq!(top.check(), Diagnostics(vec![]));
}

#[test]
fn check_descends_into_instantiated_definitions() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let mid = ModDef::new("mid");
    let s = mid.instantiate(&src, Some("s"), None);
    let d = mid.instantiate(&dst, Some("d"), None);
    d.get_port("i1").connect(&s.get_port("f"));
    let top = ModDef::new("top");
    top.instantiate(&mid, Some("m0"), None);
    top.instantiate(&mid, Some("m1"), None);
    let diagnostics = top.check();
    // `mid` is checked once, though instantiated twice.
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    assert_eq!(
        diagnostics.0[0].secondary[0].text,
        "`mid.d.i1` is `lay_pkg::flit_t`"
    );
}

#[test]
fn emit_panics_with_rendered_s0201() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    d.get_port("i1").connect(&s.get_port("f"));
    s.get_port("o").unused();
    s.get_port("h").unused();
    d.get_port("i").tieoff(0);
    d.get_port("h").tieoff(0);

    let message = panic_message(|| {
        top.emit(EmitOptions::default());
    });
    assert_eq!(message, top.check().render());
    assert!(
        message.starts_with("error[S0201]: incompatible types"),
        "{message}"
    );
}

#[test]
fn unknown_field_panics_with_s0202() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let w = lay.get_port("w");
    let message = panic_message(|| {
        w.index(1).field("flits").index(2).field("hdr.dst_idd");
    });
    assert!(
        message.starts_with("error[S0202]: no field `dst_idd` in `lay_pkg::hdr_t`"),
        "{message}"
    );
    assert!(message.contains("tests/typed/connect.rs"), "{message}");
    assert!(
        message.contains("selecting from `lay.w[1].flits[2].hdr`"),
        "{message}"
    );
    assert!(
        message.contains("available fields: `dst_id`, `len`"),
        "{message}"
    );

    // A field of an array needs an index first; a vector has no fields.
    let message = panic_message(|| {
        w.field("a");
    });
    assert!(
        message.starts_with("error[S0202]: cannot select field `a` of `lay_pkg::wrap_t [1:0]`"),
        "{message}"
    );
    assert!(
        message.contains("select an element with `index` first"),
        "{message}"
    );
    let message = panic_message(|| {
        lay.get_port("v").field("x");
    });
    assert!(
        message.starts_with("error[S0202]: no field `x` in `logic [7:0]`: it is not a struct"),
        "{message}"
    );
    // Plain bit slices and untyped ports have no fields.
    let message = panic_message(|| {
        w.slice(10, 0).field("a");
    });
    assert!(
        message
            .starts_with("error[S0202]: cannot select fields: `lay.w[10:0]` is a plain bit slice"),
        "{message}"
    );
    let p = ModDef::new("p");
    let plain = p.add_port("x", IO::Input(4));
    let message = panic_message(|| {
        plain.field("a");
    });
    assert!(
        message.starts_with("error[S0202]: cannot select fields: `p.x` has no type"),
        "{message}"
    );
}

#[test]
fn index_out_of_range_panics_with_s0203() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let w = lay.get_port("w");
    let message = panic_message(|| {
        w.index(2);
    });
    assert!(
        message.starts_with("error[S0203]: index 2 is out of range for `lay_pkg::wrap_t [1:0]`"),
        "{message}"
    );
    assert!(
        message.contains("the outermost dimension is `[1:0]`"),
        "{message}"
    );
    // Ascending `[0:3]`: 3 is valid, 4 is not.
    w.index(0).field("asc").index(3);
    let message = panic_message(|| {
        w.index(0).field("asc").index(4);
    });
    assert!(
        message.starts_with("error[S0203]: index 4 is out of range"),
        "{message}"
    );
    let message = panic_message(|| {
        w.index(0).field("a").index(0);
    });
    assert!(
        message.starts_with("error[S0203]: cannot index `logic`: it has no packed dimension"),
        "{message}"
    );
    assert!(message.contains("use `bit` or `slice`"), "{message}");
    // A struct without dimensions: no hint about vector bits.
    let message = panic_message(|| {
        w.index(0).index(0);
    });
    assert!(
        message.starts_with(
            "error[S0203]: cannot index `lay_pkg::wrap_t`: it has no packed dimension"
        ),
        "{message}"
    );
    assert!(!message.contains("use `bit` or `slice`"), "{message}");
}

#[test]
fn field_through_union_panics_with_s0204() {
    let sources = Sources::new(&[PKG, MODS]);
    let lay = sources.import("lay");
    let message = panic_message(|| {
        lay.get_port("u").field("hdr.len");
    });
    assert!(
        message
            .starts_with("error[S0204]: cannot select field `hdr` through union `lay_pkg::hdr_u`"),
        "{message}"
    );
    // The secondary label points at the port declaration.
    assert!(
        message.contains("`u` declared here as `lay_pkg::hdr_u`"),
        "{message}"
    );
}

#[test]
fn logic_vectors_connect_width_only() {
    let sources = Sources::new(&[PKG, MODS]);
    let (a, b) = (sources.import("pv_src"), sources.import("pv_dst"));
    let top = ModDef::new("top");
    let a = top.instantiate(&a, Some("a"), None);
    let b = top.instantiate(&b, Some("b"), None);
    // `[8:1]`/`[7:0]`, `wire`/`logic [0:0]`, `[3:0][1:0]`/`[7:0]`,
    // `[0:7]`/`[7:0]`, signed/unsigned, struct/vector, enum/vector.
    for port in ["o", "s", "q", "r", "g", "st", "e"] {
        let sink = if port == "o" { "i" } else { port };
        a.get_port(port).connect(&b.get_port(sink));
    }
    assert_eq!(top.check(), Diagnostics(vec![]));
    let text = top.emit(EmitOptions::default());
    assert!(text.contains("module top"), "{text}");
}

/// `top` with `src` as `s` and `dst` as `d`; `d.i1` (`flit_t`) and `s.f`
/// (`flat_t`) are equal-width structs with different layouts.
fn mismatched_pair(sources: &Sources) -> (ModDef, ModInst, ModInst) {
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    (top, s, d)
}

fn assert_one_s0201_at(top: &ModDef, line: u32) {
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    let d = &diagnostics.0[0];
    assert_eq!(d.code, codes::S0201);
    let Span::Rust { file, line: l, .. } = &d.primary.span else {
        panic!("expected a Rust span: {:?}", d.primary.span);
    };
    assert!(file.ends_with("tests/typed/connect.rs"), "{file}");
    assert_eq!(*l, line, "{}", diagnostics.render());
}

#[test]
fn pipelined_connection_is_type_checked() {
    let sources = Sources::new(&[PKG, MODS]);
    let (top, s, d) = mismatched_pair(&sources);
    let pipeline = PipelineConfig {
        clk: "clk".into(),
        depth: 1,
        inst_name: None,
    };
    let (f, i1) = (s.get_port("f"), d.get_port("i1"));
    let line = line!() + 1;
    f.connect_pipeline(&i1, pipeline);
    assert_one_s0201_at(&top, line);
}

#[test]
fn connect_through_reports_at_the_call_site() {
    let sources = Sources::new(&[PKG, MODS]);
    let (top, s, d) = mismatched_pair(&sources);
    let line = line!() + 1;
    s.get_port("f").connect_through(&d.get_port("i1"), &[], "x");
    assert_one_s0201_at(&top, line);

    // Through feedthroughs, the end-to-end pair is checked.
    let (top, s, d) = mismatched_pair(&sources);
    let thru = ModDef::new("thru");
    let t = top.instantiate(&thru, Some("t"), None);
    let (f, i1) = (s.get_port("f"), d.get_port("i1"));
    let line = line!() + 1;
    f.connect_through(&i1, &[&t], "x");
    assert_one_s0201_at(&top, line);

    // The same through `PortSlice`.
    let (top, s, d) = mismatched_pair(&sources);
    let (f, i1) = (s.get_port("f").to_port_slice(), d.get_port("i1"));
    let line = line!() + 1;
    f.connect_through(&i1, &[], "x");
    assert_one_s0201_at(&top, line);
}

#[test]
fn intf_connection_of_whole_ports_is_type_checked() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    src.def_intf(
        "bus",
        IndexMap::from([("v".to_string(), ("f".to_string(), 43, 0))]),
    );
    dst.def_intf(
        "bus",
        IndexMap::from([("v".to_string(), ("i1".to_string(), 43, 0))]),
    );
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    let line = line!() + 1;
    s.get_intf("bus").connect(&d.get_intf("bus"), false);
    assert_one_s0201_at(&top, line);
}

#[test]
fn funnel_over_typed_ends_emits() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst, ch) = (
        sources.import("src"),
        sources.import("dst"),
        sources.import("ch"),
    );
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    let fa = top.instantiate(&ch, Some("fa"), None);
    let fb = top.instantiate(&ch, Some("fb"), None);
    let mut funnel = Funnel::new(
        (fa.get_port("x_in"), fa.get_port("x_out")),
        (fb.get_port("x_in"), fb.get_port("x_out")),
    );
    funnel.connect(&s.get_port("o"), &d.get_port("i"));
    fa.get_port("x_out").unused();
    fb.get_port("x_in").tieoff(0);
    s.get_port("h").unused();
    s.get_port("f").unused();
    d.get_port("h").tieoff(0);
    d.get_port("i1").tieoff(0);
    assert_eq!(top.check(), Diagnostics(vec![]));
    top.emit(EmitOptions::default());
}

#[test]
fn check_skips_definitions_that_are_not_emitted() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let mid = ModDef::new("mid");
    let s = mid.instantiate(&src, Some("s"), None);
    let d = mid.instantiate(&dst, Some("d"), None);
    d.get_port("i1").connect(&s.get_port("f"));
    let top = ModDef::new("top");
    top.instantiate(&mid, Some("m"), None);
    for usage in [Usage::EmitNothingAndStop, Usage::EmitDefinitionAndStop] {
        mid.set_usage(usage);
        assert_eq!(top.check(), Diagnostics(vec![]));
        // Checked directly, `mid` still reports.
        assert_eq!(mid.check().0.len(), 1);
    }
    mid.set_usage(Usage::EmitDefinitionAndDescend);
    assert_eq!(top.check().0.len(), 1);
}
