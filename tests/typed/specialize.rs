// SPDX-License-Identifier: Apache-2.0

//! Typed specialization: type and named-constant overrides elaborated under
//! a generated top, coexisting specializations, and the errors S0101, S0102,
//! S0104 and S0105. Cache invalidation is a unit test in
//! `src/import/specialize.rs` (it counts elaborations).

use slang_rs::str2tmpfile;
use tempfile::NamedTempFile;
use topstitch::diagnostic::codes;
use topstitch::*;

const PKG: &str = "
package cfg_pkg;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { hdr_t hdr; logic [31:0] data; } flit_t;
  localparam int Depth = 5;
endpackage
";

const CHILD: &str = "
module child #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output T out_o
);
  localparam int L = N;
endmodule
";

/// Sources written to temp files, kept alive while their modules are used.
struct Sources {
    files: Vec<NamedTempFile>,
}

impl Sources {
    fn new(sources: &[&str]) -> Self {
        Sources {
            files: sources.iter().map(|s| str2tmpfile(s).unwrap()).collect(),
        }
    }

    fn import_with(&self, name: &str, skip_unsupported: bool, extra: &[&str]) -> ModDef {
        let paths: Vec<&str> = self
            .files
            .iter()
            .map(|f| f.path().to_str().unwrap())
            .collect();
        let cfg = ParserConfig {
            sources: &paths,
            skip_unsupported,
            extra_arguments: extra,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    }

    fn import(&self, name: &str) -> ModDef {
        self.import_with(name, false, &[])
    }
}

fn logic(dims: &[(i64, i64)]) -> PortType {
    PortType::Logic {
        signed: false,
        packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn cfg_pkg(name: &str) -> Option<QualifiedName> {
    Some(QualifiedName {
        package: Some("cfg_pkg".to_string()),
        name: name.to_string(),
    })
}

fn field(name: &str, ty: PortType) -> PortField {
    PortField {
        name: name.to_string(),
        ty,
    }
}

fn hdr(packed: &[(i64, i64)]) -> PortType {
    PortType::Struct {
        name: cfg_pkg("hdr_t"),
        signed: false,
        is_packed: true,
        fields: vec![
            field("dst_id", logic(&[(3, 0)])),
            field("len", logic(&[(7, 0)])),
        ],
        packed: packed.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn flit(packed: &[(i64, i64)]) -> PortType {
    PortType::Struct {
        name: cfg_pkg("flit_t"),
        signed: false,
        is_packed: true,
        fields: vec![field("hdr", hdr(&[])), field("data", logic(&[(31, 0)]))],
        packed: packed.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn ty(value: &str) -> ParameterValue {
    ParameterValue::Type(value.to_string())
}

fn expr(value: &str) -> ParameterValue {
    ParameterValue::Expr(value.to_string())
}

fn rust_span(line: u32) -> (String, u32) {
    (file!().to_string(), line)
}

fn primary_line(e: &ImportError) -> (String, u32) {
    match &e.diagnostics.0[0].primary.span {
        Span::Rust { file, line, .. } => (file.clone(), *line),
        other => panic!("expected a Rust span, got {other:?}"),
    }
}

#[test]
fn type_override_resolves_ports() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let spec = child
        .try_parameterize(&[("T", ty("cfg_pkg::flit_t")), ("N", 3.into())])
        .unwrap();

    assert_eq!(spec.get_name(), "child");
    assert_eq!(
        spec.get_port("in_i").port_type(),
        Some(flit(&[(2, 0)])),
        "T [N-1:0] with T = flit_t (44 bits) and N = 3"
    );
    assert_eq!(spec.get_port("in_i").io().width(), 132);
    assert_eq!(spec.get_port("out_o").port_type(), Some(flit(&[])));
    assert_eq!(spec.get_port("out_o").io().width(), 44);
    // The unspecialized module keeps its defaults.
    assert_eq!(child.get_port("in_i").port_type(), Some(logic(&[(1, 0)])));
}

#[test]
fn expr_override_resolves_named_constant() {
    let sources = Sources::new(&[PKG, CHILD]);
    let spec = sources
        .import("child")
        .try_parameterize(&[("N", expr("cfg_pkg::Depth"))])
        .unwrap();
    assert_eq!(spec.get_port("in_i").port_type(), Some(logic(&[(4, 0)])));
}

#[test]
fn type_and_expr_overrides_emit() {
    let sources = Sources::new(&[PKG, CHILD]);
    let spec = sources
        .import("child")
        .try_parameterize(&[("T", ty("cfg_pkg::flit_t")), ("N", expr("cfg_pkg::Depth"))])
        .unwrap();
    let emitted = spec.wrap(None, None).emit(EmitOptions::default());
    assert!(
        emitted.contains(
            "  child #(\n    .T(cfg_pkg::flit_t),\n    .N(cfg_pkg::Depth)\n  ) child_i (\n"
        ),
        "{emitted}"
    );
    // The wrapper's ports keep the specialization's types.
    assert!(
        emitted.contains("  input cfg_pkg::flit_t [4:0] in_i,\n  output cfg_pkg::flit_t out_o\n"),
        "{emitted}"
    );
    let files: Vec<&std::path::Path> = sources.files.iter().map(|f| f.path()).collect();
    crate::common::slang::elaborate(&files, &emitted, "child_wrapper")
        .unwrap_or_else(|e| panic!("{emitted}\n{e}"));
}

#[test]
fn two_type_specializations_coexist() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let with_flit = child
        .try_parameterize(&[("T", ty("cfg_pkg::flit_t"))])
        .unwrap();
    let with_hdr = child
        .try_parameterize(&[("T", ty("cfg_pkg::hdr_t"))])
        .unwrap();

    assert_eq!(child.specialization_key(), None);
    let flit_key = with_flit.specialization_key().unwrap();
    let hdr_key = with_hdr.specialization_key().unwrap();
    assert_ne!(flit_key, hdr_key);
    assert_ne!(flit_key.short_hash(), hdr_key.short_hash());
    assert_eq!(flit_key.short_hash().len(), 12);
    assert!(
        flit_key
            .short_hash()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );

    assert_eq!(with_flit.get_name(), "child");
    assert_eq!(with_hdr.get_name(), "child");
    assert_eq!(with_flit.get_port("out_o").port_type(), Some(flit(&[])));
    assert_eq!(with_hdr.get_port("out_o").port_type(), Some(hdr(&[])));
    // Specializing again reuses the identity.
    let again = child
        .try_parameterize(&[("T", ty("cfg_pkg::flit_t"))])
        .unwrap();
    assert_eq!(again.specialization_key(), Some(flit_key));
}

#[test]
fn unknown_parameter_reports_s0101() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");

    let (result, line) = (child.try_parameterize(&[("NN", 3)]), line!());
    let e = result.err().unwrap();
    assert_eq!(e.diagnostics.0.len(), 1);
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0101);
    assert_eq!(d.message, "`child` has no overridable parameter `NN`");
    assert_eq!(primary_line(&e), rust_span(line));
    // In declaration order.
    assert_eq!(d.notes, vec!["`child` declares `T`, `N`".to_string()]);
    assert_eq!(d.helps, vec!["did you mean `N`?".to_string()]);

    // A localparam cannot be overridden.
    let e = child.try_parameterize(&[("L", 3)]).err().unwrap();
    assert_eq!(e.diagnostics.0[0].code, codes::S0101);

    // `parameterize` panics with the rendered diagnostic.
    let (call, line) = (|| child.parameterize(&[("NN", 3)]), line!());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call))
        .err()
        .unwrap();
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(message.contains("error[S0101]"), "{message}");
    // The span is the caller of `parameterize`, not library code.
    assert!(
        message.contains(&format!("--> {}:{line}:", file!())),
        "{message}"
    );
}

#[test]
fn unknown_type_reports_s0102() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let overrides: [(&str, ParameterValue); 2] = [("N", 3.into()), ("T", ty("nopkg::flit_t"))];
    let (result, line) = (child.try_parameterize(&overrides), line!());
    let e = result.err().unwrap();
    assert_eq!(e.diagnostics.0.len(), 1);
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0102);
    assert_eq!(primary_line(&e), rust_span(line));
    assert!(
        d.notes
            .contains(&"in the override of `T`: unknown class or package 'nopkg'".to_string()),
        "{:?}",
        d.notes
    );
    assert!(
        d.notes
            .iter()
            .any(|n| n.contains(".N(32'd3), .T(nopkg::flit_t)")),
        "{:?}",
        d.notes
    );
    // The generated top is not a source: no label points into it.
    assert!(d.secondary.is_empty(), "{:?}", d.secondary);
}

#[test]
fn bad_override_expr_reports_s0105() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    for bad in [
        ty("struct packed { logic a; }"),
        ty("logic [3:0]"),
        ty("a::b::c"),
        ty(""),
        expr("cfg_pkg::Depth + 1"),
        expr("2"),
        expr("cfg_pkg::"),
    ] {
        let (result, line) = (child.try_parameterize(&[("T", bad.clone())]), line!());
        let e = result.err().unwrap();
        assert_eq!(e.diagnostics.0.len(), 1, "{bad:?}");
        assert_eq!(e.diagnostics.0[0].code, codes::S0105, "{bad:?}");
        assert_eq!(primary_line(&e), rust_span(line), "{bad:?}");
    }

    // All bad overrides are reported together.
    let e = child
        .try_parameterize(&[("T", ty("int unsigned")), ("N", expr("-1"))])
        .err()
        .unwrap();
    let codes: Vec<_> = e.diagnostics.0.iter().map(|d| d.code).collect();
    assert_eq!(codes, vec![codes::S0105, codes::S0105]);
}

#[test]
fn reparameterize_merges_overrides() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let stepwise = child
        .try_parameterize(&[("T", ty("cfg_pkg::hdr_t"))])
        .unwrap()
        .try_parameterize(&[("N", 4)])
        .unwrap();
    let at_once = child
        .try_parameterize::<ParameterValue>(&[("T", ty("cfg_pkg::hdr_t")), ("N", 4.into())])
        .unwrap();
    assert_eq!(stepwise.get_port("in_i").port_type(), Some(hdr(&[(3, 0)])));
    assert_eq!(stepwise.specialization_key(), at_once.specialization_key());

    // A later override of the same parameter replaces it in place.
    let replaced = stepwise
        .try_parameterize(&[("T", ty("cfg_pkg::flit_t"))])
        .unwrap();
    assert_eq!(replaced.get_port("in_i").port_type(), Some(flit(&[(3, 0)])));
    let emitted = replaced.wrap(None, None).emit(EmitOptions::default());
    assert!(
        emitted.contains("  child #(\n    .T(cfg_pkg::flit_t),\n    .N(32'h0000_0004)\n  )"),
        "{emitted}"
    );
}

#[test]
fn interface_port_reports_s0104_before_elaboration() {
    let iface = "interface bus_if; logic a; endinterface";
    let module = "
module with_if #(parameter int N = 1) (bus_if b, input logic [N-1:0] x);
endmodule
";
    let sources = Sources::new(&[iface, module]);
    // Imported with its interface port skipped.
    let base = sources.import_with("with_if", true, &["--allow-toplevel-iface-ports"]);
    assert_eq!(base.get_port("x").io().width(), 1);

    let e = base.try_parameterize(&[("N", 4)]).err().unwrap();
    assert_eq!(e.diagnostics.0.len(), 1, "{e}");
    assert_eq!(e.diagnostics.0[0].code, codes::S0104, "{e}");
    assert_eq!(
        e.diagnostics.0[0].message,
        "port `b` of `with_if` is an interface port"
    );
}

#[test]
fn permuted_overrides_share_key() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let tn = child
        .try_parameterize::<ParameterValue>(&[("T", ty("cfg_pkg::hdr_t")), ("N", 3.into())])
        .unwrap();
    let nt = child
        .try_parameterize::<ParameterValue>(&[("N", 3.into()), ("T", ty("cfg_pkg::hdr_t"))])
        .unwrap();
    assert_eq!(tn.specialization_key(), nt.specialization_key());
    // Emission keeps each caller's order.
    let emitted = nt.wrap(None, None).emit(EmitOptions::default());
    assert!(
        emitted.contains("  child #(\n    .N(32'h0000_0003),\n    .T(cfg_pkg::hdr_t)\n  )"),
        "{emitted}"
    );
}

#[test]
fn wrong_kind_overrides_report_s0105() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    for (name, value, text) in [
        (
            "N",
            ty("cfg_pkg::flit_t"),
            "`N` is a value parameter, not a type parameter",
        ),
        ("T", 3.into(), "`T` is a type parameter"),
        ("T", expr("cfg_pkg::Depth"), "`T` is a type parameter"),
        ("T", "logic".into(), "`T` is a type parameter"),
    ] {
        let (result, line) = (child.try_parameterize(&[(name, value.clone())]), line!());
        let e = result.err().unwrap();
        assert_eq!(e.diagnostics.0.len(), 1, "{value:?}");
        let d = &e.diagnostics.0[0];
        assert_eq!(d.code, codes::S0105, "{value:?}");
        assert_eq!(d.primary.text, text, "{value:?}");
        assert_eq!(primary_line(&e), rust_span(line), "{value:?}");
    }
}

#[test]
fn integer_override_overflow_reports_s0105() {
    let module = "
module ov #(parameter int N = 1, parameter logic [7:0] B = 0) (output logic o);
endmodule
";
    let sources = Sources::new(&[module]);
    let ov = sources.import("ov");
    assert!(ov.try_parameterize(&[("N", (1i64 << 31) - 1)]).is_ok());
    assert!(ov.try_parameterize(&[("B", 255)]).is_ok());
    for (name, too_big, text) in [
        ("N", 1i64 << 31, "signed 32-bit"),
        ("N", 1i64 << 40, "signed 32-bit"),
        ("B", 256, "unsigned 8-bit"),
    ] {
        let (result, line) = (ov.try_parameterize(&[(name, too_big)]), line!());
        let e = result.err().unwrap();
        let d = &e.diagnostics.0[0];
        assert_eq!(d.code, codes::S0105, "{too_big}");
        assert_eq!(
            d.primary.text,
            format!("`{too_big}` does not fit the {text} type of `{name}`")
        );
        assert_eq!(primary_line(&e), rust_span(line));
    }
}

#[test]
fn timescale_source_specializes() {
    let module = "`timescale 1ns/1ps
module ts #(parameter int N = 1) (output logic [N-1:0] o, input logic i);
endmodule
";
    let sources = Sources::new(&[module]);
    let spec = sources.import("ts").try_parameterize(&[("N", 4)]).unwrap();
    assert_eq!(spec.get_port("o").io().width(), 4);
}

#[test]
fn user_timescale_argument_is_kept() {
    let module = "`timescale 1ns/1ps
module ts2 #(parameter int N = 1) (output logic [N-1:0] o);
endmodule
";
    let sources = Sources::new(&[module]);
    for extra in [
        &["--timescale=1ns/1ps"][..],
        &["--timescale", "1ns/1ps"][..],
    ] {
        let spec = sources
            .import_with("ts2", false, extra)
            .try_parameterize(&[("N", 3)])
            .unwrap_or_else(|e| panic!("{extra:?}: {e}"));
        assert_eq!(spec.get_port("o").io().width(), 3);
    }
}

#[test]
fn user_mall_argument_reports_s0102() {
    let deps = tempfile::tempdir().unwrap();
    let list = deps.path().join("deps.txt");
    let list = list.to_str().unwrap();
    let sources = Sources::new(&[PKG, CHILD]);
    let joined = format!("--Mall={list}");
    for extra in [&["--Mall", list][..], &[joined.as_str()][..]] {
        let child = sources.import_with("child", false, extra);
        let (result, line) = (child.try_parameterize(&[("N", 3)]), line!());
        let e = result.err().unwrap();
        assert_eq!(e.diagnostics.0.len(), 1, "{extra:?}");
        let d = &e.diagnostics.0[0];
        assert_eq!(d.code, codes::S0102, "{extra:?}");
        assert_eq!(primary_line(&e), rust_span(line));
        assert!(
            d.notes[0].contains(&format!("contain `{}`", extra[0])),
            "{d:?}"
        );
        assert_eq!(
            d.helps,
            vec!["remove `--Mall` from `ParserConfig::extra_arguments`".to_string()]
        );
    }
}

#[test]
fn werror_config_specializes() {
    // The generated top leaves the ports unconnected, which Slang warns about.
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import_with("child", false, &["-Werror"]);
    let spec = child.try_parameterize(&[("N", 3)]).unwrap();
    assert_eq!(spec.get_port("in_i").io().width(), 3);
}

#[test]
fn bare_name_overrides_need_single_unit() {
    let unit = "localparam int K = 7;\ntypedef logic [5:0] u_t;\n";
    let module = "
module bm #(parameter int N = 1, parameter type T = logic) (output logic [N-1:0] o, input T t);
endmodule
";
    let sources = Sources::new(&[unit, module]);
    let overrides = [("N", expr("K")), ("T", ty("u_t"))];

    let single = sources.import_with("bm", false, &["--single-unit"]);
    let spec = single.try_parameterize(&overrides).unwrap();
    assert_eq!(spec.get_port("o").io().width(), 7);
    assert_eq!(spec.get_port("t").io().width(), 6);
    let emitted = spec.wrap(None, None).emit(EmitOptions::default());
    assert!(
        emitted.contains("  bm #(\n    .N(K),\n    .T(u_t)\n  )"),
        "{emitted}"
    );

    // Without it, the generated top is its own compilation unit.
    let e = sources
        .import("bm")
        .try_parameterize(&overrides)
        .err()
        .unwrap();
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0102);
    assert!(
        d.notes
            .contains(&"in the override of `N`: use of undeclared identifier 'K'".to_string()),
        "{:?}",
        d.notes
    );
    assert_eq!(
        d.helps,
        vec![
            "a bare name in an override refers to the compilation unit; use `pkg::name` or \
             `--single-unit`"
                .to_string()
        ]
    );
}

#[test]
fn wrapped_specialization_keeps_types_and_checks() {
    let swapped = "
package w_pkg;
  typedef struct packed { logic [7:0] len; logic [3:0] dst_id; } swapped_t;
endpackage
";
    let sources = Sources::new(&[PKG, swapped, CHILD]);
    let child = sources.import("child");
    let hdr = child
        .try_parameterize::<ParameterValue>(&[("T", ty("cfg_pkg::hdr_t")), ("N", 1.into())])
        .unwrap();
    let wrapper = hdr.wrap(None, None);
    assert!(wrapper.get_port("out_o").port_type().is_some());
    assert_eq!(
        wrapper.get_port("out_o").port_type(),
        hdr.get_port("out_o").port_type()
    );

    // A connection of the wrapper's typed port is still type-checked.
    let other = child
        .try_parameterize::<ParameterValue>(&[("T", ty("w_pkg::swapped_t")), ("N", 1.into())])
        .unwrap();
    let top = ModDef::new("top");
    let w = top.instantiate(&wrapper, Some("w"), None);
    let o = top.instantiate(&other, Some("o"), None);
    w.get_port("out_o").connect(&o.get_port("in_i"));
    let diagnostics = top.check();
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    assert_eq!(diagnostics.0[0].code, codes::S0201);
}
