// SPDX-License-Identifier: Apache-2.0

//! Resolved type-parameter port types, package-qualified type names, source
//! locations, interface ports and type-parameter listing.

use slang_rs::*;

const CFG_PKG: &str = "
package cfg_pkg;
  typedef struct packed { logic [3:0] a; logic [7:0] b; } flit_t;
endpackage
";

const CHILD: &str = "
module child #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output cfg_pkg::flit_t out_o
);
endmodule
";

fn elaborate(sources: &[&str], tops: &[&str], extra_arguments: &[&str]) -> serde_json::Value {
    let files: Vec<_> = sources.iter().map(|s| str2tmpfile(s).unwrap()).collect();
    let paths: Vec<&str> = files.iter().map(|f| f.path().to_str().unwrap()).collect();
    let cfg = SlangConfig {
        sources: &paths,
        tops,
        extra_arguments,
        ..Default::default()
    };
    run_slang(&cfg).unwrap()
}

fn flit_fields() -> Vec<Field> {
    vec![
        Field {
            name: "a".to_string(),
            ty: Type::Logic {
                signed: false,
                packed_dimensions: vec![Range { msb: 3, lsb: 0 }],
                unpacked_dimensions: vec![],
            },
        },
        Field {
            name: "b".to_string(),
            ty: Type::Logic {
                signed: false,
                packed_dimensions: vec![Range { msb: 7, lsb: 0 }],
                unpacked_dimensions: vec![],
            },
        },
    ]
}

#[test]
fn type_param_port_with_dims() {
    let top = "
module top;
  child #(.T(cfg_pkg::flit_t), .N(3)) u();
endmodule
";
    let value = elaborate(&[CFG_PKG, CHILD, top], &["top"], &[]);
    let ports = extract_instance_ports_from_value(&value, "top.u", false).unwrap();
    assert_eq!(ports[0].name, "in_i");
    assert_eq!(ports[0].dir, PortDir::Input);
    assert_eq!(
        ports[0].ty,
        Type::Struct {
            name: "cfg_pkg::flit_t".to_string(),
            signed: false,
            fields: flit_fields(),
            packed_dimensions: vec![Range { msb: 2, lsb: 0 }],
            unpacked_dimensions: vec![],
        }
    );
    assert_eq!(ports[0].ty.width().unwrap(), 36);
}

#[test]
fn type_param_port_default_and_packed_override() {
    // Default `T = logic` with N = 2 as a top-level module.
    let value = elaborate(&[CFG_PKG, CHILD], &["child"], &[]);
    let ports = extract_ports_from_value(&value, false);
    assert_eq!(
        ports["child"][0].ty,
        Type::Logic {
            signed: false,
            packed_dimensions: vec![Range { msb: 1, lsb: 0 }],
            unpacked_dimensions: vec![],
        }
    );

    // A packed override: use-site dimensions are the outer ones.
    let top = "
module top;
  child #(.T(logic signed [7:0]), .N(3)) u();
endmodule
";
    let value = elaborate(&[CFG_PKG, CHILD, top], &["top"], &[]);
    let ports = extract_instance_ports_from_value(&value, "top.u", false).unwrap();
    assert_eq!(
        ports[0].ty,
        Type::Logic {
            signed: true,
            packed_dimensions: vec![Range { msb: 2, lsb: 0 }, Range { msb: 7, lsb: 0 }],
            unpacked_dimensions: vec![],
        }
    );
    assert!(extract_instance_ports_from_value(&value, "top.nope", false).is_none());
}

#[test]
fn package_qualified_name() {
    let imported = "
module imp import cfg_pkg::*; (input flit_t in_i);
endmodule
";
    let value = elaborate(&[CFG_PKG, CHILD, imported], &["child", "imp"], &[]);
    let ports = extract_ports_from_value(&value, false);
    for (module, port) in [("child", 1), ("imp", 0)] {
        match &ports[module][port].ty {
            Type::Struct { name, fields, .. } => {
                assert_eq!(name, "cfg_pkg::flit_t", "{module}");
                assert_eq!(fields, &flit_fields());
            }
            other => panic!("expected struct for {module}, got {other:?}"),
        }
    }
}

fn check_child_locations(path: &str) {
    let cfg = SlangConfig {
        sources: &[path],
        tops: &["child"],
        ..Default::default()
    };
    let ports = extract_ports(&cfg, false);
    let expected_file = std::fs::canonicalize(path).unwrap();
    // CFG_PKG spans lines 1-4; CHILD's port lines are 7 and 8.
    for (port, line, column) in [(0, 7, 19), (1, 8, 26)] {
        let loc = ports["child"][port].loc.as_ref().unwrap();
        assert!(std::path::Path::new(&loc.file).is_absolute(), "{loc:?}");
        assert_eq!(std::fs::canonicalize(&loc.file).unwrap(), expected_file);
        assert_eq!((loc.line, loc.column), (line, Some(column)));
    }
}

#[test]
fn port_has_location() {
    let file = str2tmpfile(&(CFG_PKG.to_string() + CHILD)).unwrap();
    let path = file.path().to_str().unwrap();
    assert!(file.path().is_absolute());
    check_child_locations(path);
}

#[test]
fn port_location_from_relative_source_is_absolute() {
    // Keep the source in a temp dir (nothing is written to the checkout) and
    // pass it to Slang by a path relative to the current directory:
    // `../` up to the root, then the file's absolute path without the root.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("child.sv");
    std::fs::write(&file, CFG_PKG.to_string() + CHILD).unwrap();
    let file = std::fs::canonicalize(&file).unwrap();
    let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
    let mut relative = std::path::PathBuf::new();
    for _ in cwd.components().skip(1) {
        relative.push("..");
    }
    relative.push(file.strip_prefix("/").unwrap());
    assert!(relative.is_relative());
    check_child_locations(relative.to_str().unwrap());
}

#[test]
fn macro_expanded_port_has_no_column() {
    let sv = "`define PORT(n) input logic n
module mac (
  `PORT(a_i),
  input logic b_i
);
endmodule
";
    let value = elaborate(&[sv], &["mac"], &[]);
    let ports = extract_ports_from_value(&value, false);
    let a_loc = ports["mac"][0].loc.as_ref().unwrap();
    assert_eq!((a_loc.line, a_loc.column), (3, None));
    let b_loc = ports["mac"][1].loc.as_ref().unwrap();
    assert_eq!((b_loc.line, b_loc.column), (4, Some(15)));
}

const IFACE_SV: &str = "
interface bus_if; logic a; logic b; modport mst(output a, input b); endinterface
module m (bus_if.mst m_if, bus_if s_if, input logic x);
endmodule
";

#[test]
#[should_panic(expected = "Interface ports are not currently supported.")]
fn interface_port_panics_without_skip() {
    let value = elaborate(&[IFACE_SV], &["m"], &["--allow-toplevel-iface-ports"]);
    extract_ports_from_value(&value, false);
}

#[test]
fn interface_port_listed() {
    let value = elaborate(&[IFACE_SV], &["m"], &["--allow-toplevel-iface-ports"]);

    // With `skip_unsupported`, interface ports are skipped by
    // `extract_ports_from_value` and listed separately.
    let ports = extract_ports_from_value(&value, true);
    assert_eq!(ports["m"].len(), 1);
    assert_eq!(ports["m"][0].name, "x");

    let interface_ports = extract_interface_ports_from_value(&value);
    let listed = &interface_ports["m"];
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].name, "m_if");
    assert_eq!(listed[0].interface, "bus_if");
    assert_eq!(listed[0].modport.as_deref(), Some("mst"));
    let loc = listed[0].loc.as_ref().unwrap();
    assert_eq!((loc.line, loc.column), (3, Some(22)));
    assert_eq!(listed[1].name, "s_if");
    assert_eq!(listed[1].interface, "bus_if");
    assert_eq!(listed[1].modport, None);
}

#[test]
fn type_params_listed() {
    let sv = "
module tp #(parameter type A = logic, parameter int W = 1, parameter type B = bit) ();
  localparam type L = logic [W-1:0];
endmodule
";
    let value = elaborate(&[sv], &["tp"], &[]);
    let names = extract_type_parameter_names_from_value(&value);
    assert_eq!(names["tp"], vec!["A".to_string(), "B".to_string()]);

    // Type parameters are not value parameters and do not cause errors.
    let parameters = extract_parameter_defs_from_value(&value, false);
    let parameter_names: Vec<_> = parameters["tp"].iter().map(|p| p.name.as_str()).collect();
    assert_eq!(parameter_names, vec!["W"]);
}

#[test]
fn value_parameter_typed_by_type_parameter() {
    let sv = "
module vp #(parameter type T = logic [3:0], parameter T INIT = '0) (input T x);
endmodule
";
    let value = elaborate(&[sv], &["vp"], &[]);
    let parameters = extract_parameter_defs_from_value(&value, false);
    assert_eq!(parameters["vp"][0].name, "INIT");
    assert_eq!(parameters["vp"][0].ty.width().unwrap(), 4);
}

#[test]
fn two_instances_resolve_their_own_type_parameter() {
    let top = "
module top;
  child #(.T(cfg_pkg::flit_t), .N(3)) u_flit();
  child #(.T(logic [5:0]), .N(4)) u_vec();
endmodule
";
    let value = elaborate(&[CFG_PKG, CHILD, top], &["top"], &[]);
    let flit = extract_instance_ports_from_value(&value, "top.u_flit", false).unwrap();
    assert_eq!(
        flit[0].ty,
        Type::Struct {
            name: "cfg_pkg::flit_t".to_string(),
            signed: false,
            fields: flit_fields(),
            packed_dimensions: vec![Range { msb: 2, lsb: 0 }],
            unpacked_dimensions: vec![],
        }
    );
    let vec = extract_instance_ports_from_value(&value, "top.u_vec", false).unwrap();
    assert_eq!(
        vec[0].ty,
        Type::Logic {
            signed: false,
            packed_dimensions: vec![Range { msb: 3, lsb: 0 }, Range { msb: 5, lsb: 0 }],
            unpacked_dimensions: vec![],
        }
    );
}

/// Asserts a struct/union/enum keeps its package-qualified name and that the
/// `$` before unpacked dimensions is not taken as part of the name.
fn assert_named_unpacked(ty: &Type, expected_name: &str) {
    let unpacked = vec![Range { msb: 0, lsb: 1 }];
    match ty {
        Type::Struct {
            name,
            packed_dimensions,
            unpacked_dimensions,
            ..
        }
        | Type::Union {
            name,
            packed_dimensions,
            unpacked_dimensions,
            ..
        }
        | Type::Enum {
            name,
            packed_dimensions,
            unpacked_dimensions,
            ..
        } => {
            assert_eq!(name, expected_name);
            assert!(packed_dimensions.is_empty(), "{ty:?}");
            assert_eq!(unpacked_dimensions, &unpacked, "{ty:?}");
        }
        other => panic!("expected an aggregate, got {other:?}"),
    }
}

#[test]
fn unpacked_dimensions_after_qualified_name() {
    let pkg = "
package agg_pkg;
  typedef struct packed { logic [3:0] a; } s_t;
  typedef union packed { logic [3:0] x; logic [3:0] y; } u_t;
  typedef enum logic [1:0] { A, B, C } e_t;
endpackage
";
    let plain = "
module plain (
  input agg_pkg::s_t s_i [2],
  input agg_pkg::u_t u_i [2],
  input agg_pkg::e_t e_i [2]
);
endmodule
";
    let param = "
module param #(parameter type T = logic) (input T up_i [2]);
endmodule
module top;
  param #(.T(agg_pkg::s_t)) u_s();
  param #(.T(agg_pkg::u_t)) u_u();
  param #(.T(agg_pkg::e_t)) u_e();
endmodule
";
    let value = elaborate(&[pkg, plain, param], &["plain", "top"], &[]);
    let ports = extract_ports_from_value(&value, false);
    for (port, name) in ["agg_pkg::s_t", "agg_pkg::u_t", "agg_pkg::e_t"]
        .iter()
        .enumerate()
    {
        assert_named_unpacked(&ports["plain"][port].ty, name);
    }
    for (instance, name) in [
        ("top.u_s", "agg_pkg::s_t"),
        ("top.u_u", "agg_pkg::u_t"),
        ("top.u_e", "agg_pkg::e_t"),
    ] {
        let ports = extract_instance_ports_from_value(&value, instance, false).unwrap();
        assert_named_unpacked(&ports[0].ty, name);
    }
}

fn logic(signed: bool, msb: i32) -> Type {
    Type::Logic {
        signed,
        packed_dimensions: vec![Range { msb, lsb: 0 }],
        unpacked_dimensions: vec![],
    }
}

#[test]
fn integral_atom_types_parse() {
    let sv = "
module atoms (
  input byte b_i,
  input shortint s_i,
  input integer i_i,
  input integer unsigned iu_i,
  input time t_i,
  input byte unsigned bu_i
);
endmodule
";
    let value = elaborate(&[sv], &["atoms"], &[]);
    let ports = extract_ports_from_value(&value, false);
    let types: Vec<(&str, &Type)> = ports["atoms"]
        .iter()
        .map(|p| (p.name.as_str(), &p.ty))
        .collect();
    assert_eq!(
        types,
        vec![
            ("b_i", &logic(true, 7)),
            ("s_i", &logic(true, 15)),
            ("i_i", &logic(true, 31)),
            ("iu_i", &logic(false, 31)),
            ("t_i", &logic(false, 63)),
            ("bu_i", &logic(false, 7)),
        ]
    );
}

#[test]
fn signed_packed_struct_and_atom_fields_parse() {
    let sv = "
package sp;
  typedef struct packed signed { byte a; integer b; shortint c; } s_t;
endpackage
module sm (input sp::s_t x);
endmodule
";
    let value = elaborate(&[sv], &["sm"], &[]);
    let ports = extract_ports_from_value(&value, false);
    let Type::Struct { fields, .. } = &ports["sm"][0].ty else {
        panic!("expected a struct, got {:?}", ports["sm"][0].ty);
    };
    let fields: Vec<(&str, &Type)> = fields.iter().map(|f| (f.name.as_str(), &f.ty)).collect();
    assert_eq!(
        fields,
        vec![
            ("a", &logic(true, 7)),
            ("b", &logic(true, 31)),
            ("c", &logic(true, 15)),
        ]
    );
    assert_eq!(ports["sm"][0].ty.width().unwrap(), 56);
    // The typedef name replaces Slang's internal name, as for unsigned structs.
    let Type::Struct { name, .. } = &ports["sm"][0].ty else {
        unreachable!()
    };
    assert_eq!(name, "sp::s_t");
}

#[test]
fn struct_and_union_signing() {
    let sv = "
package sg;
  typedef struct packed signed { logic [3:0] a; } ss_t;
  typedef struct packed unsigned { logic [3:0] a; } su_t;
  typedef struct packed { logic [3:0] a; } sp_t;
  typedef union packed signed { logic [3:0] a; logic [3:0] b; } us_t;
  typedef union packed { logic [3:0] a; logic [3:0] b; } up_t;
endpackage
module sgm (input sg::ss_t ss, input sg::su_t su, input sg::sp_t sp,
            input sg::us_t us, input sg::up_t up);
endmodule
";
    let value = elaborate(&[sv], &["sgm"], &[]);
    let ports = extract_ports_from_value(&value, false);
    let signing: Vec<(&str, &str, bool)> = ports["sgm"]
        .iter()
        .map(|p| match &p.ty {
            Type::Struct { name, signed, .. } | Type::Union { name, signed, .. } => {
                (p.name.as_str(), name.as_str(), *signed)
            }
            other => panic!("expected a struct or union, got {other:?}"),
        })
        .collect();
    assert_eq!(
        signing,
        vec![
            ("ss", "sg::ss_t", true),
            ("su", "sg::su_t", false),
            ("sp", "sg::sp_t", false),
            ("us", "sg::us_t", true),
            ("up", "sg::up_t", false),
        ]
    );
}

#[test]
fn enum_variant_signedness() {
    let sv = "
package ep;
  typedef enum { X, Y } int_e;
  typedef enum logic signed [3:0] { SA = 1, SB = -2 } signed_e;
  typedef enum logic [3:0] { UA = 1, UB = 2 } unsigned_e;
endpackage
module em (input ep::int_e i_i, input ep::signed_e s_i, input ep::unsigned_e u_i);
endmodule
";
    let value = elaborate(&[sv], &["em"], &[]);
    let ports = extract_ports_from_value(&value, false);
    let signedness: Vec<(&str, Vec<bool>)> = ports["em"]
        .iter()
        .map(|p| match &p.ty {
            Type::Enum { variants, .. } => {
                (p.name.as_str(), variants.iter().map(|v| v.signed).collect())
            }
            other => panic!("expected an enum, got {other:?}"),
        })
        .collect();
    assert_eq!(
        signedness,
        vec![
            ("i_i", vec![true, true]),
            ("s_i", vec![true, true]),
            ("u_i", vec![false, false]),
        ]
    );
}

#[test]
fn unparsable_port_types_listed() {
    let sv = "
module np (
  input real r_i,
  input logic x,
  input string s_i
);
endmodule
";
    let value = elaborate(&[sv], &["np"], &[]);
    let ports = extract_instance_ports_checked_from_value(&value, "np").unwrap();
    // Declaration order, parsed and unparsed interleaved.
    type Listed<'a> = (&'a str, Option<(&'a str, u32, Option<u32>)>);
    let listed: Vec<Listed> = ports
        .iter()
        .map(|port| match port {
            Ok(port) => (port.name.as_str(), None),
            Err(u) => {
                let loc = u.loc.as_ref().unwrap();
                (
                    u.name.as_str(),
                    Some((u.type_text.as_str(), loc.line, loc.column)),
                )
            }
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("r_i", Some(("real", 3, Some(14)))),
            ("x", None),
            ("s_i", Some(("string", 5, Some(16)))),
        ]
    );
    assert!(extract_instance_ports_checked_from_value(&value, "nope").is_none());
}

#[test]
fn overridable_parameters_in_declaration_order() {
    let sv = "
module vp #(parameter type T = logic, parameter int W = 1, localparam int L = 2,
            parameter type U = bit) ();
  localparam int B = 3;
  parameter int Q = 4;
endmodule
";
    let value = elaborate(&[sv], &["vp"], &[]);
    let parameters = extract_overridable_parameters_from_value(&value);
    // A body `parameter` in a module with a parameter port list is a localparam.
    assert_eq!(
        parameters["vp"],
        vec![
            ("T".to_string(), ParameterKind::Type),
            ("W".to_string(), ParameterKind::Value),
            ("U".to_string(), ParameterKind::Type),
        ]
    );
}

#[test]
fn instance_parameter_defs_apply_overrides() {
    let child = "
module pchild #(parameter int W = 8, parameter logic [W-1:0] INIT = '0) ();
endmodule
";
    let top = "
module top;
  pchild #(.W(20)) u_wide();
endmodule
";
    let value = elaborate(&[child, top], &["top"], &[]);
    let defs = extract_instance_parameter_defs_from_value(&value, "top.u_wide").unwrap();
    let widths: Vec<_> = defs
        .iter()
        .map(|p| (p.name.as_str(), p.ty.width().unwrap()))
        .collect();
    assert_eq!(widths, vec![("W", 32), ("INIT", 20)]);
    assert!(extract_instance_parameter_defs_from_value(&value, "top.nope").is_none());
}

#[test]
fn slang_version_names_slang() {
    assert!(slang_version().unwrap().starts_with("slang version "));
}

#[test]
fn absolute_path_normalizes() {
    let dir = std::path::Path::new("/a/b/c");
    assert_eq!(absolute_path(dir, "../../x/./y.sv"), "/a/x/y.sv");
    assert_eq!(absolute_path(dir, "/abs/z.sv"), "/abs/z.sv");
}
