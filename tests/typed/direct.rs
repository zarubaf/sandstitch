// SPDX-License-Identifier: Apache-2.0

//! Importing a module directly with parameter overrides
//! (`ModDef::try_from_verilog_parameterized`), including modules whose
//! default parameters do not elaborate.

use std::path::PathBuf;
use topstitch::diagnostic::codes;
use topstitch::*;

use crate::common::slang::{Sources, elaborate_manifest};

/// `T` defaults to `logic`, but `m` reads member `v` of its input: `m`
/// elaborates only with `T` overridden by a struct with a member `v`.
const FAILING_DEFAULTS: &str = "
package p;
  typedef struct packed { logic v; logic [3:0] d; } t;
  typedef struct packed { logic [1:0] w; } u;
endpackage
module m #(parameter type T = logic, parameter int N = 1) (
  input T x,
  input logic [N-1:0] z,
  output logic y
);
  assign y = x.v;
endmodule
";

/// Elaborates with its defaults.
const CHILD: &str = "
package cfg_pkg;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  localparam int Depth = 5;
endpackage
module child #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output T out_o
);
endmodule
";

fn config<'a>(paths: &'a [&'a str]) -> ParserConfig<'a> {
    ParserConfig {
        sources: paths,
        ..Default::default()
    }
}

fn ty(value: &str) -> ParameterValue {
    ParameterValue::Type(value.to_string())
}

fn logic(dims: &[(i64, i64)]) -> PortType {
    PortType::Logic {
        signed: false,
        packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn field(name: &str, ty: PortType) -> PortField {
    PortField {
        name: name.to_string(),
        ty,
    }
}

fn primary_line(e: &ImportError) -> (String, u32) {
    match &e.diagnostics.0[0].primary.span {
        Span::Rust { file, line, .. } => (file.clone(), *line),
        other => panic!("expected a Rust span, got {other:?}"),
    }
}

fn no_overrides() -> &'static [(&'static str, ParameterValue)] {
    &[]
}

/// A top instantiating `module` as `u`, with every input tied off and every
/// output marked unused.
fn wrap_in_top(module: &ModDef) -> ModDef {
    let top = ModDef::new("top");
    let inst = top.instantiate(module, Some("u"), None);
    for port in inst.get_ports(None) {
        match port.io() {
            IO::Input(_) => port.tieoff(0),
            _ => port.unused(),
        }
    }
    top
}

#[test]
fn failing_defaults_import_with_type_override() {
    let sources = Sources::new(&[FAILING_DEFAULTS]);
    let paths = sources.paths();
    let cfg = config(&paths);

    // With its defaults, `m` does not elaborate.
    let e = ModDef::try_from_verilog_with_config("m", &cfg)
        .err()
        .unwrap();
    assert_eq!(e.diagnostics.0[0].code, codes::S0102, "{e}");

    let overrides: [(&str, ParameterValue); 2] = [("T", ty("p::t")), ("N", 3.into())];
    let m = ModDef::try_from_verilog_parameterized("m", &cfg, &overrides)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(m.get_name(), "m");
    let names: Vec<String> = m
        .get_ports(None)
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    assert_eq!(names, ["x", "z", "y"]);
    assert_eq!(
        m.get_port("x").port_type(),
        Some(PortType::Struct {
            name: Some(QualifiedName {
                package: Some("p".to_string()),
                name: "t".to_string(),
            }),
            signed: false,
            fields: vec![field("v", logic(&[])), field("d", logic(&[(3, 0)]))],
            packed: vec![],
        })
    );
    assert_eq!(m.get_port("x").io().width(), 5);
    assert_eq!(m.get_port("z").port_type(), Some(logic(&[(2, 0)])));
    assert!(matches!(m.get_port("y").io(), IO::Output(1)));
    assert!(m.specialization_key().is_some());

    // The emitted instance carries the overrides, and the emitted RTL
    // elaborates with the manifest's sources.
    let top = wrap_in_top(&m);
    let emitted = top.emit(EmitOptions::default());
    assert!(
        emitted.contains("  m #(\n    .T(p::t),\n    .N(32'h0000_0003)\n  ) u ("),
        "{emitted}"
    );
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![PathBuf::from(sources.paths()[0])]);
    assert_eq!(
        manifest.specializations,
        vec![(
            "top.u".to_string(),
            m.specialization_key().unwrap().short_hash()
        )]
    );
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn same_result_as_import_then_parameterize() {
    let sources = Sources::new(&[CHILD]);
    let paths = sources.paths();
    let cfg = config(&paths);
    let overrides: [(&str, ParameterValue); 2] = [("T", ty("cfg_pkg::hdr_t")), ("N", 3.into())];
    let direct = ModDef::try_from_verilog_parameterized("child", &cfg, &overrides).unwrap();
    let stepwise = ModDef::try_from_verilog_with_config("child", &cfg)
        .unwrap()
        .try_parameterize(&overrides)
        .unwrap();

    let ports = |m: &ModDef| -> Vec<(String, Option<PortType>, String)> {
        m.get_ports(None)
            .iter()
            .map(|p| (p.name().to_string(), p.port_type(), format!("{:?}", p.io())))
            .collect()
    };
    assert_eq!(ports(&direct), ports(&stepwise));
    // Independently: `T [N-1:0]` with a 12-bit `hdr_t` and N = 3.
    assert_eq!(direct.get_port("in_i").io().width(), 36);
    assert!(direct.specialization_key().is_some());
    assert_eq!(direct.specialization_key(), stepwise.specialization_key());

    let direct_top = wrap_in_top(&direct);
    let stepwise_top = wrap_in_top(&stepwise);
    let emitted = direct_top.emit(EmitOptions::default());
    assert!(
        emitted.contains("  child #(\n    .T(cfg_pkg::hdr_t),\n    .N(32'h0000_0003)\n  ) u ("),
        "{emitted}"
    );
    assert_eq!(emitted, stepwise_top.emit(EmitOptions::default()));
    assert_eq!(direct_top.manifest(), stepwise_top.manifest());
    assert_eq!(
        direct_top.manifest().sources,
        vec![PathBuf::from(sources.paths()[0])]
    );
}

#[test]
fn reparameterize_merges_overrides() {
    let sources = Sources::new(&[FAILING_DEFAULTS]);
    let paths = sources.paths();
    let cfg = config(&paths);
    let merged = ModDef::try_from_verilog_parameterized("m", &cfg, &[("T", ty("p::t"))])
        .unwrap()
        .try_parameterize(&[("N", 4)])
        .unwrap();
    let overrides: [(&str, ParameterValue); 2] = [("T", ty("p::t")), ("N", 4.into())];
    let at_once = ModDef::try_from_verilog_parameterized("m", &cfg, &overrides).unwrap();
    // `T` is kept from the first step.
    assert_eq!(merged.get_port("x").io().width(), 5);
    assert_eq!(merged.get_port("z").port_type(), Some(logic(&[(3, 0)])));
    assert_eq!(merged.specialization_key(), at_once.specialization_key());
}

#[test]
fn no_overrides_is_a_plain_import() {
    // Failing defaults: the same S0102 as the plain import.
    let failing = Sources::new(&[FAILING_DEFAULTS]);
    let paths = failing.paths();
    let cfg = config(&paths);
    let direct = ModDef::try_from_verilog_parameterized("m", &cfg, no_overrides())
        .err()
        .unwrap();
    let plain = ModDef::try_from_verilog_with_config("m", &cfg)
        .err()
        .unwrap();
    assert_eq!(direct.diagnostics.0[0].code, codes::S0102);
    assert_eq!(
        direct.diagnostics.0[0].message,
        "Slang failed to elaborate `m`"
    );
    assert_eq!(direct.to_string(), plain.to_string());

    // Elaborating defaults: an unspecialized module.
    let child = Sources::new(&[CHILD]);
    let paths = child.paths();
    let m =
        ModDef::try_from_verilog_parameterized("child", &config(&paths), no_overrides()).unwrap();
    assert_eq!(m.specialization_key(), None);
    assert_eq!(m.get_port("in_i").port_type(), Some(logic(&[(1, 0)])));
}

#[test]
fn unknown_parameter_reports_s0101() {
    let sources = Sources::new(&[FAILING_DEFAULTS]);
    let paths = sources.paths();
    let cfg = config(&paths);
    let overrides: [(&str, ParameterValue); 2] = [("T", ty("p::t")), ("NN", 3.into())];
    let (result, line) = (
        ModDef::try_from_verilog_parameterized("m", &cfg, &overrides),
        line!(),
    );
    let e = result.err().unwrap();
    assert_eq!(e.diagnostics.0.len(), 1, "{e}");
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0101);
    assert_eq!(d.message, "`m` has no overridable parameter `NN`");
    // The parameters are known although the defaults do not elaborate.
    assert_eq!(d.notes, vec!["`m` declares `T`, `N`".to_string()]);
    assert_eq!(d.helps, vec!["did you mean `N`?".to_string()]);
    assert_eq!(primary_line(&e), (file!().to_string(), line - 1));
}

#[test]
fn wrong_kind_reports_s0105_and_bad_type_s0102() {
    let sources = Sources::new(&[FAILING_DEFAULTS]);
    let paths = sources.paths();
    let cfg = config(&paths);
    let e = ModDef::try_from_verilog_parameterized("m", &cfg, &[("T", 3)])
        .err()
        .unwrap();
    assert_eq!(e.diagnostics.0.len(), 1, "{e}");
    assert_eq!(e.diagnostics.0[0].code, codes::S0105, "{e}");
    assert_eq!(e.diagnostics.0[0].primary.text, "`T` is a type parameter");

    // `p::u` has no member `v`: the specialization itself fails.
    let overrides = [("T", ty("p::u"))];
    let (result, line) = (
        ModDef::try_from_verilog_parameterized("m", &cfg, &overrides),
        line!(),
    );
    let e = result.err().unwrap();
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0102, "{e}");
    assert_eq!(
        d.message,
        "Slang failed to elaborate a specialization of `m`"
    );
    assert_eq!(primary_line(&e), (file!().to_string(), line - 1));
    assert!(
        d.secondary
            .iter()
            .any(|l| l.text == "no member named 'v' in 'T' (aka 'u')"),
        "{e}"
    );
}

#[test]
fn interface_port_reports_s0104() {
    // The interface port is found although the defaults do not elaborate.
    let module = "
interface bus_if; logic a; endinterface
module with_if #(parameter type T = logic) (bus_if b, input T x, output logic y);
  assign y = x.v;
endmodule
";
    let sources = Sources::new(&[FAILING_DEFAULTS, module]);
    let paths = sources.paths();
    let overrides = [("T", ty("p::t"))];
    let e = ModDef::try_from_verilog_parameterized("with_if", &config(&paths), &overrides)
        .err()
        .unwrap();
    assert_eq!(e.diagnostics.0.len(), 1, "{e}");
    assert_eq!(e.diagnostics.0[0].code, codes::S0104, "{e}");
    assert_eq!(
        e.diagnostics.0[0].message,
        "port `b` of `with_if` is an interface port"
    );
}

#[test]
fn syntax_error_in_parameter_list_reports_s0102() {
    // `;` for `,`: Slang's recovered AST makes `M` and `T` local, which must
    // not turn into "no overridable parameter `T`".
    let typo = "
module m #(parameter int N = 1; parameter int M = 2, parameter type T = logic) (
  input T x
);
endmodule
";
    let sources = Sources::new(&[typo]);
    let paths = sources.paths();
    let overrides = [("T", ty("logic"))];
    let e = ModDef::try_from_verilog_parameterized("m", &config(&paths), &overrides)
        .err()
        .unwrap();
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0102, "{e}");
    assert_eq!(
        d.message,
        "Slang failed to elaborate a specialization of `m`"
    );
    assert!(d.secondary.iter().any(|l| l.text == "expected ')'"), "{e}");
}

#[test]
fn slang_parameters_are_kept_by_specializations() {
    let sources = Sources::new(&[CHILD]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        parameters: &[("N", "4")],
        ..config(&paths)
    };
    // The plain import elaborates with `-G N=4`.
    let base = ModDef::try_from_verilog_with_config("child", &cfg).unwrap();
    assert_eq!(base.get_port("in_i").port_type(), Some(logic(&[(3, 0)])));

    // Specializing keeps N = 4 and emits it.
    let hdr = [("T", ty("cfg_pkg::hdr_t"))];
    let stepwise = base.try_parameterize(&hdr).unwrap();
    assert_eq!(stepwise.get_port("in_i").io().width(), 48);
    let emitted = wrap_in_top(&stepwise).emit(EmitOptions::default());
    assert!(
        emitted.contains("  child #(\n    .N(32'h0000_0004),\n    .T(cfg_pkg::hdr_t)\n  ) u ("),
        "{emitted}"
    );

    // Directly: the same specialization.
    let direct = ModDef::try_from_verilog_parameterized("child", &cfg, &hdr).unwrap();
    assert_eq!(direct.specialization_key(), stepwise.specialization_key());
    assert_eq!(direct.get_port("in_i").io().width(), 48);

    // Explicit overrides win, in both flows.
    let overrides: [(&str, ParameterValue); 2] = [("T", ty("cfg_pkg::hdr_t")), ("N", 2.into())];
    let explicit = ModDef::try_from_verilog_parameterized("child", &cfg, &overrides).unwrap();
    assert_eq!(explicit.get_port("in_i").io().width(), 24);
    let replaced = stepwise.try_parameterize(&[("N", 2)]).unwrap();
    assert_eq!(replaced.specialization_key(), explicit.specialization_key());

    // A name is a named constant.
    let cfg = ParserConfig {
        parameters: &[("N", "cfg_pkg::Depth")],
        ..config(&paths)
    };
    let depth = ModDef::try_from_verilog_parameterized("child", &cfg, &hdr).unwrap();
    assert_eq!(depth.get_port("in_i").io().width(), 60);
}

#[test]
fn unsupported_slang_parameter_reports_s0105() {
    let sources = Sources::new(&[CHILD]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        parameters: &[("N", "1+1")],
        ..config(&paths)
    };
    let hdr = [("T", ty("cfg_pkg::hdr_t"))];
    let (result, line) = (
        ModDef::try_from_verilog_parameterized("child", &cfg, &hdr),
        line!(),
    );
    let e = result.err().unwrap();
    assert_eq!(e.diagnostics.0.len(), 1, "{e}");
    let d = &e.diagnostics.0[0];
    assert_eq!(d.code, codes::S0105, "{e}");
    assert_eq!(
        d.message,
        "unsupported `-G` override for parameter `N` of `child`"
    );
    assert_eq!(primary_line(&e), (file!().to_string(), line - 1));

    // Also when specializing a plain import.
    let base = ModDef::try_from_verilog_with_config("child", &cfg).unwrap();
    let e = base.try_parameterize(&hdr).err().unwrap();
    assert_eq!(e.diagnostics.0[0].code, codes::S0105, "{e}");

    // An explicit override replaces the `-G` value.
    let overrides: [(&str, ParameterValue); 2] = [("T", ty("cfg_pkg::hdr_t")), ("N", 2.into())];
    let m = base.try_parameterize(&overrides).unwrap();
    assert_eq!(m.get_port("in_i").io().width(), 24);
}
