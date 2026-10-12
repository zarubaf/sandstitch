// SPDX-License-Identifier: Apache-2.0

//! Typed import: resolved port types, declaration spans and the import
//! errors for ports that cannot be represented (S0103, S0104).

use num_bigint::BigInt;
use slang_rs::str2tmpfile;
use tempfile::NamedTempFile;
use topstitch::diagnostic::codes;
use topstitch::*;

const PKG: &str = "
package cfg_pkg;
  typedef struct packed { logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { hdr_t hdr; logic [31:0] data; } flit_t;
  typedef union packed { logic [5:0] wide; struct packed { logic [1:0] hi; logic [3:0] lo; } split; } u_t;
  typedef enum logic [1:0] { IDLE, BUSY, WAIT = 3 } state_t;
  typedef enum logic signed [1:0] { NEG = -1, POS = 1 } sign_t;
endpackage
";

const IFACE: &str = "
interface bus_if; logic a; logic b; modport mst(output a, input b); endinterface
";

/// Sources written to temp files; kept alive for the spans' files.
struct Sources {
    files: Vec<NamedTempFile>,
}

impl Sources {
    fn new(sources: &[&str]) -> Self {
        Sources {
            files: sources.iter().map(|s| str2tmpfile(s).unwrap()).collect(),
        }
    }

    fn paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .map(|f| f.path().to_str().unwrap())
            .collect()
    }

    /// Path of the last source (the module under test).
    fn last(&self) -> String {
        self.files
            .last()
            .unwrap()
            .path()
            .to_str()
            .unwrap()
            .to_string()
    }

    fn import(&self, name: &str) -> Result<ModDef, ImportError> {
        let paths = self.paths();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::try_from_verilog_with_config(name, &cfg)
    }
}

fn sv(file: &str, line: u32, column: u32, len: u32) -> Span {
    Span::Sv {
        file: file.to_string(),
        line,
        column,
        len,
    }
}

fn logic(signed: bool, dims: &[(i64, i64)]) -> PortType {
    PortType::Logic {
        signed,
        packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn field(name: &str, ty: PortType) -> PortField {
    PortField {
        name: name.to_string(),
        ty,
    }
}

fn cfg_pkg(name: &str) -> Option<QualifiedName> {
    Some(QualifiedName {
        package: Some("cfg_pkg".to_string()),
        name: name.to_string(),
    })
}

fn flit(packed: &[(i64, i64)]) -> PortType {
    PortType::Struct {
        name: cfg_pkg("flit_t"),
        signed: false,
        is_packed: true,
        fields: vec![
            field(
                "hdr",
                PortType::Struct {
                    name: cfg_pkg("hdr_t"),
                    signed: false,
                    is_packed: true,
                    fields: vec![
                        field("dst_id", logic(false, &[(3, 0)])),
                        field("len", logic(false, &[(7, 0)])),
                    ],
                    packed: vec![],
                },
            ),
            field("data", logic(false, &[(31, 0)])),
        ],
        packed: packed.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

fn state(packed: &[(i64, i64)]) -> PortType {
    PortType::Enum {
        name: cfg_pkg("state_t"),
        base: Box::new(logic(false, &[(1, 0)])),
        variants: vec![
            ("IDLE".to_string(), BigInt::from(0)),
            ("BUSY".to_string(), BigInt::from(1)),
            ("WAIT".to_string(), BigInt::from(3)),
        ],
        packed: packed.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
    }
}

const TYPED: &str = "
module m import cfg_pkg::*; (
  input cfg_pkg::flit_t [1:0] flit_i,
  output logic signed [3:0] s_o,
  input state_t st_i,
  output struct packed { logic a; logic [2:0] b; } anon_o
);
endmodule
";

#[test]
fn struct_port_keeps_fields_and_qualified_name() {
    let sources = Sources::new(&[PKG, TYPED]);
    let m = sources.import("m").unwrap();

    assert_eq!(m.get_port("flit_i").port_type(), Some(flit(&[(1, 0)])));
    assert_eq!(m.get_port("s_o").port_type(), Some(logic(true, &[(3, 0)])));
    // Imported through `import cfg_pkg::*`, still package-qualified.
    assert_eq!(m.get_port("st_i").port_type(), Some(state(&[])));
    assert_eq!(
        m.get_port("anon_o").port_type(),
        Some(PortType::Struct {
            name: None,
            signed: false,
            is_packed: true,
            fields: vec![
                field("a", logic(false, &[])),
                field("b", logic(false, &[(2, 0)])),
            ],
            packed: vec![],
        })
    );
    assert_eq!(
        m.get_port("flit_i").port_type().unwrap().to_string(),
        "cfg_pkg::flit_t [1:0]"
    );
}

#[test]
fn width_equals_flat_width() {
    let sv_src = "
module w import cfg_pkg::*; (
  input cfg_pkg::flit_t [1:0] flit_i,
  input u_t u_i,
  input int i_i,
  input logic [1:0][7:0] two_d_i,
  output state_t [2:0] states_o,
  input logic bit_i
);
endmodule
";
    let sources = Sources::new(&[PKG, sv_src]);
    let w = sources.import("w").unwrap();
    // (4 + 8 + 32) * 2, max(6, 2 + 4), 32, 2 * 8, 2 * 3, 1
    let expected = [
        ("flit_i", 88),
        ("u_i", 6),
        ("i_i", 32),
        ("two_d_i", 16),
        ("states_o", 6),
        ("bit_i", 1),
    ];
    for (name, width) in expected {
        let port = w.get_port(name);
        assert_eq!(port.io().width(), width, "IO width of {name}");
        assert_eq!(
            port.port_type().unwrap().width(),
            width,
            "type width of {name}"
        );
    }
    assert!(matches!(w.get_port("states_o").io(), IO::Output(6)));
    assert_eq!(w.get_port("states_o").port_type(), Some(state(&[(2, 0)])));
    assert_eq!(w.get_port("i_i").port_type(), Some(logic(true, &[(31, 0)])));
}

#[test]
fn negative_enum_value_gives_signed_base() {
    let sv_src = "
module e (input cfg_pkg::sign_t s_i);
endmodule
";
    let sources = Sources::new(&[PKG, sv_src]);
    let e = sources.import("e").unwrap();
    assert_eq!(
        e.get_port("s_i").port_type(),
        Some(PortType::Enum {
            name: cfg_pkg("sign_t"),
            base: Box::new(logic(true, &[(1, 0)])),
            variants: vec![
                ("NEG".to_string(), BigInt::from(-1)),
                ("POS".to_string(), BigInt::from(1)),
            ],
            packed: vec![],
        })
    );
}

#[test]
fn source_span_of_def_and_inst_ports() {
    let sources = Sources::new(&[PKG, TYPED]);
    let m = sources.import("m").unwrap();
    let file = sources.last();

    // Line 1 of TYPED is empty; `flit_i` starts in column 31 of line 3.
    assert_eq!(m.get_port("flit_i").source_span(), sv(&file, 3, 31, 6));
    assert_eq!(m.get_port("st_i").source_span(), sv(&file, 5, 17, 4));

    let top = ModDef::new("top");
    let inst = top.instantiate(&m, Some("u"), None);
    assert_eq!(inst.get_port("flit_i").port_type(), Some(flit(&[(1, 0)])));
    assert_eq!(inst.get_port("flit_i").source_span(), sv(&file, 3, 31, 6));

    let plain = top.add_port("plain", IO::Input(3));
    assert_eq!(plain.port_type(), None);
    assert_eq!(plain.source_span(), Span::None);
}

#[test]
fn unpacked_port_reports_s0103() {
    let sv_src = "
module u (
  input logic [7:0] mem [0:3],
  output cfg_pkg::flit_t fl_o [2],
  input logic x
);
endmodule
";
    let sources = Sources::new(&[PKG, sv_src]);
    let err = sources
        .import("u")
        .err()
        .expect("unpacked ports are rejected");
    let file = sources.last();
    let diagnostics = &err.diagnostics.0;
    assert_eq!(diagnostics.len(), 2, "{}", err);

    assert_eq!(diagnostics[0].code, codes::S0103);
    assert_eq!(diagnostics[0].severity, Severity::Error);
    assert_eq!(
        diagnostics[0].message,
        "port `mem` of `u` has unpacked dimensions"
    );
    assert_eq!(diagnostics[0].primary.span, sv(&file, 3, 21, 3));

    assert_eq!(diagnostics[1].code, codes::S0103);
    assert_eq!(
        diagnostics[1].message,
        "port `fl_o` of `u` has unpacked dimensions"
    );
    assert_eq!(diagnostics[1].primary.span, sv(&file, 4, 26, 4));
}

#[test]
fn skip_unsupported_skips_unpacked_port() {
    let sv_src = "
module u (input logic [7:0] mem [0:3], input logic x);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        skip_unsupported: true,
        ..Default::default()
    };
    let u = ModDef::try_from_verilog_with_config("u", &cfg).unwrap();
    let names: Vec<String> = u
        .get_ports(None)
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    assert_eq!(names, vec!["x".to_string()]);
}

const IFACE_MODULE: &str = "
module i (
  bus_if.mst m_if,
  bus_if s_if,
  input logic x
);
endmodule
";

fn assert_interface_errors(err: &ImportError, file: &str) {
    let diagnostics = &err.diagnostics.0;
    assert_eq!(diagnostics.len(), 2, "{err}");
    assert_eq!(diagnostics[0].code, codes::S0104);
    assert_eq!(
        diagnostics[0].message,
        "port `m_if` of `i` is an interface port"
    );
    assert_eq!(diagnostics[0].primary.span, sv(file, 3, 14, 4));
    assert_eq!(
        diagnostics[0].primary.text,
        "`bus_if.mst` port is not supported"
    );
    assert_eq!(diagnostics[1].code, codes::S0104);
    assert_eq!(
        diagnostics[1].message,
        "port `s_if` of `i` is an interface port"
    );
    assert_eq!(diagnostics[1].primary.span, sv(file, 4, 10, 4));
}

#[test]
fn interface_port_reports_s0104() {
    let sources = Sources::new(&[IFACE, IFACE_MODULE]);
    let err = sources
        .import("i")
        .err()
        .expect("interface ports are rejected");
    assert_interface_errors(&err, &sources.last());

    // Same with the module selected as top.
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        tops: &["i"],
        ..Default::default()
    };
    let err = ModDef::try_from_verilog_with_config("i", &cfg)
        .err()
        .unwrap();
    assert_interface_errors(&err, &sources.last());
}

#[test]
fn interface_port_reports_s0104_when_top_elaborates() {
    // With the flag, the module elaborates as a top in the main pass; the
    // interface ports are still reported, unless explicitly skipped.
    let sources = Sources::new(&[IFACE, IFACE_MODULE]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        extra_arguments: &["--allow-toplevel-iface-ports"],
        ..Default::default()
    };
    let err = ModDef::try_from_verilog_with_config("i", &cfg)
        .err()
        .unwrap();
    assert_interface_errors(&err, &sources.last());

    let cfg = ParserConfig {
        skip_unsupported: true,
        ..cfg
    };
    let i = ModDef::try_from_verilog_with_config("i", &cfg).unwrap();
    let names: Vec<String> = i
        .get_ports(None)
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    assert_eq!(names, vec!["x".to_string()]);
}

#[test]
fn panicking_import_panics_with_rendered_diagnostic() {
    let sv_src = "
module u (input logic [7:0] mem [0:3]);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let rendered = sources.import("u").err().unwrap().to_string();
    assert!(
        rendered.starts_with("error[S0103]: port `mem` of `u` has unpacked dimensions"),
        "{rendered}"
    );
    // The snippet points at the port name.
    assert!(
        rendered.contains("^^^ unpacked dimensions are not supported"),
        "{rendered}"
    );

    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    let panic = std::panic::catch_unwind(|| ModDef::from_verilog_with_config("u", &cfg))
        .err()
        .expect("from_verilog_with_config panics");
    let message = panic
        .downcast_ref::<String>()
        .expect("panic message is a String");
    assert_eq!(message, &rendered);
}

#[test]
fn missing_module_reports_s0102() {
    let sources = Sources::new(&["module a; endmodule"]);
    let err = sources.import("b").err().unwrap();
    assert_eq!(err.diagnostics.0.len(), 1);
    assert_eq!(err.diagnostics.0[0].code, codes::S0102);
    assert_eq!(err.diagnostics.0[0].message, "cannot elaborate `b`");
}

#[test]
fn slang_error_reports_s0102_with_span() {
    let sv_src = "
module s (
  input missing_pkg::t_t x
);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let err = sources.import("s").err().unwrap();
    let d = &err.diagnostics.0[0];
    assert_eq!(d.code, codes::S0102);
    assert_eq!(d.message, "cannot elaborate `s`");
    let Span::Sv { file, line, .. } = &d.primary.span else {
        panic!("expected an SV span, got {:?}", d.primary.span);
    };
    assert_eq!((file.as_str(), *line), (sources.last().as_str(), 3));
    assert!(d.primary.text.contains("missing_pkg"), "{}", d.primary.text);
    // The located message is the label only, not repeated as a note.
    assert_eq!(d.notes, Vec::<String>::new());
}

#[test]
fn integral_atom_types_import() {
    let sv_src = "
module a (
  input byte by_i,
  input shortint sh_i,
  input integer in_i,
  input integer unsigned iu_i,
  input time t_i,
  input bit [3:0] b_i
);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let a = sources.import("a").unwrap();
    let expected = [
        ("by_i", logic(true, &[(7, 0)]), 8),
        ("sh_i", logic(true, &[(15, 0)]), 16),
        ("in_i", logic(true, &[(31, 0)]), 32),
        ("iu_i", logic(false, &[(31, 0)]), 32),
        ("t_i", logic(false, &[(63, 0)]), 64),
        ("b_i", logic(false, &[(3, 0)]), 4),
    ];
    for (name, ty, width) in expected {
        let port = a.get_port(name);
        assert_eq!(port.port_type(), Some(ty), "type of {name}");
        assert_eq!(port.io().width(), width, "width of {name}");
    }
}

#[test]
fn signed_packed_struct_with_atom_fields_imports() {
    let sv_src = "
package sp;
  typedef struct packed signed { byte a; integer b; shortint c; } s_t;
endpackage
module s (input sp::s_t x);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let s = sources.import("s").unwrap();
    assert_eq!(
        s.get_port("x").port_type(),
        Some(PortType::Struct {
            name: Some(QualifiedName {
                package: Some("sp".into()),
                name: "s_t".into(),
            }),
            signed: true,
            is_packed: true,
            fields: vec![
                field("a", logic(true, &[(7, 0)])),
                field("b", logic(true, &[(31, 0)])),
                field("c", logic(true, &[(15, 0)])),
            ],
            packed: vec![],
        })
    );
    assert_eq!(s.get_port("x").io().width(), 56);
}

#[test]
fn non_integral_ports_report_s0103() {
    let sv_src = "
module n (
  input real r_i,
  input shortreal sr_i,
  input logic ok_i, input logic m_i [2],
  input string s_i,
  input event e_i
);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let err = sources
        .import("n")
        .err()
        .expect("non-integral ports are rejected");
    let file = sources.last();
    let got: Vec<(&str, &str, Span)> = err
        .diagnostics
        .0
        .iter()
        .map(|d| (d.code.0, d.message.as_str(), d.primary.span.clone()))
        .collect();
    assert_eq!(
        got,
        vec![
            (
                "S0103",
                "port `r_i` of `n` has unsupported type `real`",
                sv(&file, 3, 14, 3)
            ),
            (
                "S0103",
                "port `sr_i` of `n` has unsupported type `shortreal`",
                sv(&file, 4, 19, 4)
            ),
            // Unparsable and unpacked ports interleave in declaration order.
            (
                "S0103",
                "port `m_i` of `n` has unpacked dimensions",
                sv(&file, 5, 33, 3)
            ),
            (
                "S0103",
                "port `s_i` of `n` has unsupported type `string`",
                sv(&file, 6, 16, 3)
            ),
            (
                "S0103",
                "port `e_i` of `n` has unsupported type `event`",
                sv(&file, 7, 15, 3)
            ),
        ]
    );

    // Skipped only when asked to.
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        skip_unsupported: true,
        ..Default::default()
    };
    let n = ModDef::try_from_verilog_with_config("n", &cfg).unwrap();
    let names: Vec<String> = n
        .get_ports(None)
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    assert_eq!(names, vec!["ok_i".to_string()]);
}

const ENUMS: &str = "
package ep;
  typedef enum { X, Y } int_e;
  typedef enum logic signed [3:0] { A = 1, B = 2 } signed_e;
  typedef enum logic [3:0] { A2 = 1, B2 = 2 } unsigned_e;
  typedef struct packed { signed_e st; } s_signed_t;
  typedef struct packed { unsigned_e st; } s_unsigned_t;
endpackage
module en (
  input ep::int_e i_i,
  input ep::signed_e s_i,
  input ep::unsigned_e u_i,
  input ep::s_signed_t ss_i,
  input ep::s_unsigned_t su_i
);
endmodule
";

#[test]
fn enum_base_signedness() {
    let sources = Sources::new(&[ENUMS]);
    let en = sources.import("en").unwrap();
    assert_eq!(
        en.get_port("i_i").port_type(),
        Some(PortType::Enum {
            name: Some(QualifiedName {
                package: Some("ep".into()),
                name: "int_e".into(),
            }),
            base: Box::new(logic(true, &[(31, 0)])),
            variants: vec![("X".into(), BigInt::from(0)), ("Y".into(), BigInt::from(1))],
            packed: vec![],
        })
    );
    let ty = |name: &str| en.get_port(name).port_type().unwrap();
    let PortType::Enum { base, .. } = ty("s_i") else {
        panic!("expected an enum");
    };
    assert_eq!(*base, logic(true, &[(3, 0)]));

    // The base is compared before the variants.
    assert_eq!(
        ty("s_i").equivalent(&ty("u_i")),
        Err(TypeDifference {
            path: "".into(),
            left: "logic signed [3:0]".into(),
            right: "logic [3:0]".into(),
        })
    );
    assert_eq!(
        ty("ss_i").equivalent(&ty("su_i")),
        Err(TypeDifference {
            path: "st".into(),
            left: "logic signed [3:0]".into(),
            right: "logic [3:0]".into(),
        })
    );
}

#[test]
fn macro_expanded_port_span_has_column_zero() {
    let sv_src = "
`define PORT(n) input logic [3:0] n
module mc (
  `PORT(a),
  input logic b
);
endmodule
";
    let sources = Sources::new(&[sv_src]);
    let mc = sources.import("mc").unwrap();
    let file = sources.last();
    assert_eq!(mc.get_port("a").source_span(), sv(&file, 4, 0, 0));
    assert_eq!(mc.get_port("b").source_span(), sv(&file, 5, 15, 1));
}

#[test]
fn all_from_verilog_reports_interface_ports() {
    let sources = Sources::new(&[IFACE, IFACE_MODULE]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        extra_arguments: &["--allow-toplevel-iface-ports"],
        ..Default::default()
    };
    let panic = std::panic::catch_unwind(|| ModDef::all_from_verilog_with_config(&cfg))
        .err()
        .expect("interface ports are rejected");
    let message = panic.downcast_ref::<String>().unwrap();
    assert!(
        message.starts_with("error[S0104]: port `m_if` of `i` is an interface port"),
        "{message}"
    );
}

#[test]
fn parameterized_ports_keep_types() {
    let sv_src = "
module p #(parameter int W = 4) (
  input logic [W-1:0] d,
  input cfg_pkg::flit_t f
);
endmodule
";
    let sources = Sources::new(&[PKG, sv_src]);
    let p = sources.import("p").unwrap();
    assert_eq!(p.get_port("d").port_type(), Some(logic(false, &[(3, 0)])));

    let p8 = p.parameterize(&[("W", 8)]);
    assert_eq!(p8.get_port("d").port_type(), Some(logic(false, &[(7, 0)])));
    assert_eq!(p8.get_port("d").io().width(), 8);
    assert_eq!(p8.get_port("f").port_type(), Some(flit(&[])));
    assert_eq!(
        p8.get_port("f").source_span(),
        sv(&sources.last(), 4, 25, 1)
    );
}

#[test]
fn hierarchy_import_keeps_types() {
    let sv_src = "
module leaf (input cfg_pkg::flit_t f);
endmodule
module h (input cfg_pkg::flit_t f);
  leaf u (.f(f));
endmodule
";
    let sources = Sources::new(&[PKG, sv_src]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        include_hierarchy: true,
        ..Default::default()
    };
    let h = ModDef::try_from_verilog_with_config("h", &cfg).unwrap();
    assert_eq!(h.get_port("f").port_type(), Some(flit(&[])));
    assert_eq!(h.get_port("f").source_span(), sv(&sources.last(), 4, 33, 1));
}

#[test]
fn all_from_verilog_is_sorted_by_name() {
    let sources = Sources::new(&["module zeta (input logic z); endmodule
module alpha (input logic a); endmodule
module mid (input byte m); endmodule"]);
    let paths = sources.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    let mod_defs = ModDef::all_from_verilog_with_config(&cfg);
    let names: Vec<String> = mod_defs.iter().map(|m| m.get_name()).collect();
    assert_eq!(names, vec!["alpha", "mid", "zeta"]);
    assert_eq!(
        mod_defs[1].get_port("m").port_type(),
        Some(logic(true, &[(7, 0)]))
    );
}

const SIGNING: &str = "
package sg;
  typedef struct packed signed { logic [3:0] a; } ss_t;
  typedef struct packed unsigned { logic [3:0] a; } su_t;
  typedef struct packed { logic [3:0] a; } sp_t;
  typedef union packed signed { logic [3:0] a; logic [3:0] b; } us_t;
  typedef union packed { logic [3:0] a; logic [3:0] b; } up_t;
  typedef struct packed { ss_t inner; } outer_s_t;
  typedef struct packed { sp_t inner; } outer_p_t;
endpackage
module sgm (
  input sg::ss_t ss, input sg::su_t su, input sg::sp_t sp,
  input sg::us_t us, input sg::up_t up,
  input sg::outer_s_t os, input sg::outer_p_t op,
  input struct packed signed { logic [1:0] x; } anon
);
endmodule
";

#[test]
fn struct_and_union_signing_is_compared() {
    let sources = Sources::new(&[SIGNING]);
    let m = sources.import("sgm").unwrap();
    let ty = |name: &str| m.get_port(name).port_type().unwrap();
    let difference = |path: &str, left: &str, right: &str| TypeDifference {
        path: path.into(),
        left: left.into(),
        right: right.into(),
    };

    // A plain packed struct is unsigned.
    assert_eq!(ty("su").equivalent(&ty("sp")), Ok(()));
    assert_eq!(
        ty("ss").equivalent(&ty("su")),
        Err(difference("", "sg::ss_t (signed)", "sg::su_t (unsigned)"))
    );
    assert_eq!(
        ty("sp").equivalent(&ty("ss")),
        Err(difference("", "sg::sp_t (unsigned)", "sg::ss_t (signed)"))
    );
    assert_eq!(
        ty("us").equivalent(&ty("up")),
        Err(difference("", "sg::us_t (signed)", "sg::up_t (unsigned)"))
    );
    assert_eq!(
        ty("os").equivalent(&ty("op")),
        Err(difference(
            "inner",
            "sg::ss_t (signed)",
            "sg::sp_t (unsigned)"
        ))
    );
    assert_eq!(
        ty("anon").to_string(),
        "struct packed signed {logic [1:0] x;}"
    );
}
