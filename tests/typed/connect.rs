// SPDX-License-Identifier: Apache-2.0

//! Typed connections: field and index selections, type checks of
//! connections (S0201), `connect_bits`, and the selection errors
//! (S0202, S0203, S0204, S0205).

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

const UNPACKED: &str = "
package up_pkg;
  typedef struct { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_p_t;
endpackage
module up_src (output up_pkg::hdr_t o, output up_pkg::hdr_p_t p);
endmodule
module up_dst (input up_pkg::hdr_t i, input up_pkg::hdr_p_t q);
endmodule
module up_vec (input logic [11:0] v);
endmodule
module up_anon_src (output struct { logic [3:0] a; } o);
endmodule
module up_anon_dst (input struct { logic [3:0] a; } i);
endmodule
";

#[test]
fn unpacked_struct_imports_as_unpacked() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let src = sources.import("up_src");
    let Some(PortType::Struct {
        is_packed, fields, ..
    }) = src.get_port("o").port_type()
    else {
        panic!("expected a struct");
    };
    assert!(!is_packed);
    assert_eq!(fields.len(), 2);
    let Some(PortType::Struct { is_packed, .. }) = src.get_port("p").port_type() else {
        panic!("expected a struct");
    };
    assert!(is_packed);
}

#[test]
fn whole_unpacked_struct_connects_and_emits() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let (src, dst) = (sources.import("up_src"), sources.import("up_dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("i"));
    s.get_port("p").connect(&d.get_port("q"));
    assert!(top.check().0.is_empty(), "{}", top.check().render());
    // A flat vector cannot connect to an unpacked struct port; the emitted
    // RTL elaborates, so the net is typed.
    let emitted = sources.emit_checked(&top);
    assert!(emitted.contains("  wire up_pkg::hdr_t s_o;\n"), "{emitted}");
}

#[test]
fn packed_and_unpacked_structs_are_not_equivalent() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let src = sources.import("up_src");
    let (unpacked, packed) = (
        src.get_port("o").port_type().unwrap(),
        src.get_port("p").port_type().unwrap(),
    );
    assert_eq!(unpacked.width(), packed.width());
    assert_eq!(
        unpacked.equivalent(&packed),
        Err(TypeDifference {
            path: String::new(),
            left: "up_pkg::hdr_t (unpacked)".into(),
            right: "up_pkg::hdr_p_t (packed)".into(),
        })
    );

    // Connected, they are reported by `check`.
    let dst = sources.import("up_dst");
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("q"));
    s.get_port("p").connect(&d.get_port("i"));
    // Each connection is S0201; each unpacked end is also S0206, as its
    // net would be flat.
    let diagnostics = top.check();
    let codes_found: Vec<Code> = diagnostics.0.iter().map(|d| d.code).collect();
    assert_eq!(
        codes_found,
        [codes::S0206, codes::S0206, codes::S0201, codes::S0201],
        "{}",
        diagnostics.render()
    );
}

#[test]
fn field_of_unpacked_struct_panics_with_s0205() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let src = sources.import("up_src");
    let message = panic_message(|| {
        src.get_port("o").field("len");
    });
    assert!(
        message.starts_with(
            "error[S0205]: cannot select field `len` of unpacked struct `up_pkg::hdr_t`"
        ),
        "{message}"
    );
    // A packed struct of the same members selects fine.
    assert_eq!(range(&src.get_port("p").field("len")), (7, 0));
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

    // Through one feedthrough hop, the end-to-end pair is checked.
    let top = ModDef::new("top2");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    let thru = ModDef::new("thru");
    let t = top.instantiate(&thru, Some("t"), None);
    let (sb, db) = (s.get_intf("bus"), d.get_intf("bus"));
    let line = line!() + 1;
    sb.connect_through(&db, &[&t], "x", false);
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
    for usage in [
        Usage::EmitNothingAndStop,
        Usage::EmitDefinitionAndStop,
        Usage::EmitStubAndStop,
    ] {
        mid.set_usage(usage);
        assert_eq!(top.check(), Diagnostics(vec![]));
        // Checked directly, `mid` still reports.
        assert_eq!(mid.check().0.len(), 1);
    }
    mid.set_usage(Usage::EmitDefinitionAndDescend);
    assert_eq!(top.check().0.len(), 1);
}

/// Checks that `top` reports S0206 exactly for `ports`, the first with
/// `note` first, and that `emit` panics with it. `old` is Slang's result for
/// the RTL emission produced before the check; it must fail with
/// `slang_error`.
fn assert_s0206(
    top: &ModDef,
    ports: &[&str],
    note: &str,
    old: Result<(), String>,
    slang_error: &str,
) {
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), ports.len(), "{}", diagnostics.render());
    for (d, port) in diagnostics.0.iter().zip(ports) {
        assert_eq!(d.code, codes::S0206);
        assert!(
            d.message
                .ends_with(&format!("port `{port}` cannot be connected as bits")),
            "{}",
            d.message
        );
    }
    assert_eq!(diagnostics.0[0].notes[0], note);
    let message = panic_message(|| {
        top.emit(EmitOptions::default());
    });
    assert!(message.starts_with("error[S0206]"), "{message}");
    let slang = old.unwrap_err();
    assert!(slang.contains(slang_error), "{slang}");
}

#[test]
fn unpacked_struct_to_vector_reports_s0206() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let (src, vec) = (sources.import("up_src"), sources.import("up_vec"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let v = top.instantiate(&vec, Some("v"), None);
    s.get_port("o").connect(&v.get_port("v"));
    s.get_port("p").unused();
    assert_s0206(
        &top,
        &["top.s.o"],
        "it is connected to `top.v.v` of type `logic [11:0]`",
        sources.elaborate(
            "module top;\n  wire [11:0] s_o;\n  up_src s (\n    .o(s_o),\n    .p()\n  );\n  \
         up_vec v (\n    .v(s_o)\n  );\nendmodule\n",
            "top",
        ),
        "value of type 'hdr_t' cannot be assigned",
    );

    // Through bit slices.
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let v = top.instantiate(&vec, Some("v"), None);
    s.get_port("o")
        .slice(11, 4)
        .connect(&v.get_port("v").slice(7, 0));
    s.get_port("o").slice(3, 0).unused();
    v.get_port("v").slice(11, 8).tieoff(0);
    s.get_port("p").unused();
    assert_s0206(
        &top,
        &["top.s.o"],
        "only bits [11:4] of it are connected",
        sources.elaborate(
            "module top;\n  wire [11:0] s_o;\n  up_src s (\n    .o({s_o[11:4], s_o[3:0]}),\n    \
         .p()\n  );\n  up_vec v (\n    .v({4'h0, s_o[11:4]})\n  );\nendmodule\n",
            "top",
        ),
        "value of type 'hdr_t' cannot be assigned",
    );
}

#[test]
fn anonymous_unpacked_struct_reports_s0206() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let (src, dst) = (sources.import("up_anon_src"), sources.import("up_anon_dst"));
    assert!(
        src.get_port("o")
            .port_type()
            .unwrap()
            .equivalent(&dst.get_port("i").port_type().unwrap())
            .is_ok()
    );
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("i"));
    assert_s0206(
        &top,
        &["top.s.o", "top.d.i"],
        "`struct {logic [3:0] a;}` is not named with its package, so no net can be declared with it",
        sources.elaborate(
            "module top;\n  wire [3:0] s_o;\n  up_anon_src s (\n    .o(s_o)\n  );\n  \
         up_anon_dst d (\n    .i(s_o)\n  );\nendmodule\n",
            "top",
        ),
        "value of type 'struct{logic[3:0] a}' cannot be assigned",
    );
}

#[test]
fn unpacked_struct_tieoff_reports_s0206() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let dst = sources.import("up_dst");
    let top = ModDef::new("top");
    let d = top.instantiate(&dst, Some("d"), None);
    d.get_port("i").tieoff(0);
    // A packed struct takes a literal.
    d.get_port("q").tieoff(0);
    assert_s0206(
        &top,
        &["top.d.i"],
        "it is tied off",
        sources.elaborate(
            "module top;\n  up_dst d (\n    .i(12'h000),\n    .q(12'h000)\n  );\nendmodule\n",
            "top",
        ),
        "no implicit conversion from 'bit[11:0]' to 'hdr_t'",
    );
}

const NOMINAL_PKG: &str = "
package nm_pkg;
  typedef struct { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef union { logic [1:0] a; logic [2:0] b; } u_t;
endpackage
package nm2_pkg;
  typedef struct { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
endpackage
package nm_alias_pkg;
  typedef nm_pkg::hdr_t hdr_alias_t;
endpackage
";

/// Modules re-emitted or stubbed from the model, in a file of their own so
/// that the emitted RTL can be elaborated without it.
const NOMINAL_SRC: &str = "
module nm_src (output nm_pkg::hdr_t o);
endmodule
module nm_ua (output nm_pkg::u_t x);
endmodule
";

const NOMINAL_DST: &str = "
module nm_dst (input nm_pkg::hdr_t i);
endmodule
module nm_ub (input nm_pkg::u_t y);
endmodule
module nm_other (output nm2_pkg::hdr_t o);
endmodule
module nm_alias (output nm_alias_pkg::hdr_alias_t o);
endmodule
";

/// The nominal fixture, and Slang's result for `emitted` built without the
/// file of the modules emitted from the model.
struct Nominal(crate::common::slang::Sources);

impl Nominal {
    fn new() -> Self {
        Nominal(crate::common::slang::Sources::new(&[
            NOMINAL_PKG,
            NOMINAL_SRC,
            NOMINAL_DST,
        ]))
    }

    fn import(&self, name: &str) -> ModDef {
        self.0.import(name)
    }

    fn elaborate(&self, emitted: &str) -> Result<(), String> {
        let files = [self.0.files[0].path(), self.0.files[2].path()];
        crate::common::slang::elaborate(&files, emitted, "top")
    }
}

#[test]
fn stub_with_tied_off_unpacked_port_connects_typed() {
    // A stub declares its ports with their types, whatever ties them off.
    let sources = Nominal::new();
    let (src, dst) = (sources.import("nm_src"), sources.import("nm_dst"));
    src.set_usage(Usage::EmitStubAndStop);
    src.get_port("o").tieoff(0);
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("i"));
    assert!(!top.check().has_errors(), "{}", top.check().render());
    let emitted = top.emit(EmitOptions::default());
    assert!(
        emitted.contains("module nm_src(\n  output nm_pkg::hdr_t o\n);"),
        "{emitted}"
    );
    sources
        .elaborate(&emitted)
        .unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}

#[test]
fn re_emitted_definition_with_flat_unpacked_port_reports_s0206() {
    // `unpacked_union_tieoff_stays_flat` (tests/typed/emit.rs) emits `ua`
    // with a flat port; instantiated and connected typed in a parent, that
    // flat port meets a net of the union type.
    let sources = Nominal::new();
    let (ua, ub) = (sources.import("nm_ua"), sources.import("nm_ub"));
    ua.set_usage(Usage::EmitDefinitionAndDescend);
    ua.get_port("x").tieoff(0);
    // On its own, as the root, the flat port is fine.
    assert!(!ua.check().has_errors(), "{}", ua.check().render());
    let top = ModDef::new("top");
    let a = top.instantiate(&ua, Some("a"), None);
    let b = top.instantiate(&ub, Some("b"), None);
    a.get_port("x").connect(&b.get_port("y"));
    assert_s0206(
        &top,
        &["nm_ua.x"],
        "it is tied off",
        sources.elaborate(
            "module nm_ua(\n  output wire [2:0] x\n);\n  assign x = 3'h0;\nendmodule\n\
             module top;\n  wire nm_pkg::u_t a_x;\n  nm_ua a (\n    .x(a_x)\n  );\n  \
             nm_ub b (\n    .y(a_x)\n  );\nendmodule\n",
        ),
        "cannot be assigned",
    );
}

#[test]
fn unpacked_structs_of_different_names_report_s0206() {
    let sources = Nominal::new();
    let (other, dst) = (sources.import("nm_other"), sources.import("nm_dst"));
    let (a, b) = (
        other.get_port("o").port_type().unwrap(),
        dst.get_port("i").port_type().unwrap(),
    );
    assert_eq!(a.equivalent(&b), Ok(()));
    let top = ModDef::new("top");
    let s = top.instantiate(&other, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("i"));
    assert_s0206(
        &top,
        &["top.s.o", "top.d.i"],
        "it is connected to `top.d.i` of type `nm_pkg::hdr_t`, a different type with the same \
         layout; unpacked types connect only to the same named type",
        sources.elaborate(
            "module top;\n  wire nm2_pkg::hdr_t s_o;\n  nm_other s (\n    .o(s_o)\n  );\n  \
             nm_dst d (\n    .i(s_o)\n  );\nendmodule\n",
        ),
        "no implicit conversion from 'nm2_pkg::hdr_t' to 'nm_pkg::hdr_t'",
    );
}

#[test]
fn unpacked_alias_is_reported_although_it_connects() {
    // Known limitation: Slang reports a port of a typedef alias under the
    // alias's name, so the alias and the type it names look like different
    // types. SystemVerilog connects them; S0206 is reported anyway.
    let sources = Nominal::new();
    let (alias, dst) = (sources.import("nm_alias"), sources.import("nm_dst"));
    assert_eq!(
        alias.get_port("o").port_type().unwrap().to_string(),
        "nm_alias_pkg::hdr_alias_t"
    );
    let top = ModDef::new("top");
    let s = top.instantiate(&alias, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    s.get_port("o").connect(&d.get_port("i"));
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 2, "{}", diagnostics.render());
    assert!(diagnostics.0.iter().all(|d| d.code == codes::S0206));
    sources
        .elaborate(
            "module top;\n  wire nm_alias_pkg::hdr_alias_t s_o;\n  nm_alias s (\n    .o(s_o)\n  \
             );\n  nm_dst d (\n    .i(s_o)\n  );\nendmodule\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn wrapping_unpacked_ports_keeps_them_typed() {
    let sources = crate::common::slang::Sources::new(&[UNPACKED]);
    let wrapper = sources.import("up_src").wrap(None, None);
    assert!(
        !wrapper.check().has_errors(),
        "{}",
        wrapper.check().render()
    );
    let emitted = sources.emit_checked(&wrapper);
    assert!(
        emitted.contains("  output up_pkg::hdr_t o,\n  output up_pkg::hdr_p_t p\n"),
        "{emitted}"
    );
}
