// SPDX-License-Identifier: Apache-2.0

//! The typed design: port types as the shim walks them.

use std::io::Write as _;

use num_bigint::BigInt;
use slang_sys::{
    AstJson, DesignQuery, Field, Port, PortDir, Range, Request, Stage, Type, UnsupportedPort,
    Variant, run,
};

/// The ports of top `top` in a design made of `text`.
fn ports(text: &str, top: &str) -> Vec<Result<Port, UnsupportedPort>> {
    let mut file = tempfile::Builder::new().suffix(".sv").tempfile().unwrap();
    file.write_all(text.as_bytes()).unwrap();
    let r = run(&Request {
        args: vec![
            file.path().to_str().unwrap().to_string(),
            "--top".to_string(),
            top.to_string(),
        ],
        stage: Stage::Elaborate,
        ast_json: AstJson::None,
        design: DesignQuery::Structure,
    })
    .unwrap();
    assert!(r.ok, "{}", r.rendered);
    let design = r.design.expect("a design");
    assert_eq!(design.tops().collect::<Vec<_>>(), [top]);
    design.ports(top).expect("the top's ports").to_vec()
}

/// The type of the supported port `name`.
fn ty(ports: &[Result<Port, UnsupportedPort>], name: &str) -> Type {
    match ports.iter().find(|p| match p {
        Ok(p) => p.name == name,
        Err(p) => p.name == name,
    }) {
        Some(Ok(p)) => p.ty.clone(),
        other => panic!("port {name}: {other:?}"),
    }
}

/// The unsupported port `name`.
fn unsupported(ports: &[Result<Port, UnsupportedPort>], name: &str) -> UnsupportedPort {
    match ports.iter().find(|p| match p {
        Ok(p) => p.name == name,
        Err(p) => p.name == name,
    }) {
        Some(Err(p)) => p.clone(),
        other => panic!("port {name}: {other:?}"),
    }
}

fn range(msb: i32, lsb: i32) -> Range {
    Range { msb, lsb }
}

fn logic(signed: bool, packed: &[Range], unpacked: &[Range]) -> Type {
    Type::Logic {
        signed,
        packed_dimensions: packed.to_vec(),
        unpacked_dimensions: unpacked.to_vec(),
    }
}

fn field(name: &str, ty: Type) -> Field {
    Field {
        name: name.to_string(),
        ty,
    }
}

const PKG: &str = "package p; typedef struct packed { logic [3:0] a; logic b; } s_t; endpackage\n";

/// `p::s_t` with `packed` dimensions.
fn s_t(packed: &[Range]) -> Type {
    Type::Struct {
        name: "p::s_t".to_string(),
        signed: false,
        packed: true,
        fields: vec![
            field("a", logic(false, &[range(3, 0)], &[])),
            field("b", logic(false, &[], &[])),
        ],
        packed_dimensions: packed.to_vec(),
        unpacked_dimensions: vec![],
    }
}

fn struct_name(ty: &Type) -> &str {
    match ty {
        Type::Struct { name, .. } | Type::Union { name, .. } | Type::Enum { name, .. } => name,
        other => panic!("not a named type: {other:?}"),
    }
}

#[test]
fn packed_struct_named_by_package_alias() {
    let ports = ports(&format!("{PKG}module m(input p::s_t x); endmodule\n"), "m");
    assert_eq!(ports.len(), 1);
    let Ok(port) = &ports[0] else {
        panic!("{:?}", ports[0]);
    };
    assert_eq!(port.dir, PortDir::Input);
    assert_eq!(port.name, "x");
    assert_eq!(port.ty, s_t(&[]));
    let loc = port.loc.as_ref().expect("a location");
    assert_eq!((loc.line, loc.column), (2, Some(23)));
}

#[test]
fn typedef_dimensions_outermost_first() {
    let ports = ports(
        "typedef logic [7:0] byte_t;\n\
         module m(input byte_t [3:0] p, input byte_t q [2]); endmodule\n",
        "m",
    );
    assert_eq!(
        ty(&ports, "p"),
        logic(false, &[range(3, 0), range(7, 0)], &[])
    );
    assert_eq!(
        ty(&ports, "q"),
        logic(false, &[range(7, 0)], &[range(0, 1)])
    );
}

#[test]
fn array_alias_keeps_element_name() {
    let ports = ports(
        &format!("{PKG}typedef p::s_t [1:0] pair_t;\nmodule m(input pair_t x); endmodule\n"),
        "m",
    );
    assert_eq!(ty(&ports, "x"), s_t(&[range(1, 0)]));
}

#[test]
fn two_aliases_same_struct() {
    let ports = ports(
        &format!(
            "{PKG}module m(x, y);\n\
             typedef p::s_t a_t; typedef p::s_t b_t;\n\
             input a_t x; input b_t y;\nendmodule\n"
        ),
        "m",
    );
    assert_eq!(struct_name(&ty(&ports, "x")), "m.a_t");
    assert_eq!(struct_name(&ty(&ports, "y")), "m.b_t");
}

#[test]
fn type_parameter_is_transparent() {
    let ports = ports(
        &format!(
            "{PKG}module c #(parameter type T = p::s_t)(x, y);\n\
             typedef T [1:0] tt;\n\
             input T x; input tt y;\nendmodule\n"
        ),
        "c",
    );
    assert_eq!(ty(&ports, "x"), s_t(&[]));
    assert_eq!(ty(&ports, "y"), s_t(&[range(1, 0)]));
}

#[test]
fn anonymous_struct_name() {
    let ports = ports(
        "module m(input struct packed { logic a; } x); endmodule\n",
        "m",
    );
    let ty = ty(&ports, "x");
    let name = struct_name(&ty);
    let id = name
        .strip_prefix("m.s$")
        .unwrap_or_else(|| panic!("{name}"));
    assert!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
        "{name}"
    );
}

#[test]
fn enum_variants() {
    let ports = ports(
        "typedef enum logic signed [3:0] {A = -2, B, C = 7} e_t;\n\
         typedef enum logic [69:0] {X = 70'h2_0000_0000_0000_0001, Y} w_t;\n\
         module m(input e_t e, input w_t w); endmodule\n",
        "m",
    );
    let variant = |name: &str, width: usize, value: BigInt, signed: bool| Variant {
        name: name.to_string(),
        width,
        value,
        signed,
    };
    assert_eq!(
        ty(&ports, "e"),
        Type::Enum {
            name: "e_t".to_string(),
            variants: vec![
                variant("A", 4, BigInt::from(-2), true),
                variant("B", 4, BigInt::from(-1), true),
                variant("C", 4, BigInt::from(7), true),
            ],
            packed_dimensions: vec![],
            unpacked_dimensions: vec![],
        }
    );
    let big: BigInt = (BigInt::from(1) << 65u32) + 1;
    assert!(big > BigInt::from(u64::MAX));
    assert_eq!(
        ty(&ports, "w"),
        Type::Enum {
            name: "w_t".to_string(),
            variants: vec![
                variant("X", 70, big.clone(), false),
                variant("Y", 70, big + 1, false),
            ],
            packed_dimensions: vec![],
            unpacked_dimensions: vec![],
        }
    );
}

#[test]
fn enum_with_xz_is_unsupported() {
    let ports = ports(
        "typedef enum logic [1:0] {A = 2'b0x, B = 2'b11} e_t;\n\
         module m(input e_t e); endmodule\n",
        "m",
    );
    let port = unsupported(&ports, "e");
    assert_eq!(port.error, "enum values with X or Z bits are not supported");
    assert_eq!(port.type_text, "e_t");
}

#[test]
fn unpacked_struct_port() {
    let ports = ports(
        "typedef struct { logic [1:0] a; } u_t;\nmodule m(input u_t x); endmodule\n",
        "m",
    );
    assert_eq!(
        ty(&ports, "x"),
        Type::Struct {
            name: "u_t".to_string(),
            signed: false,
            packed: false,
            fields: vec![field("a", logic(false, &[range(1, 0)], &[]))],
            packed_dimensions: vec![],
            unpacked_dimensions: vec![],
        }
    );
}

#[test]
fn int_and_byte() {
    let ports = ports(
        "module m(input int i, input byte unsigned b); endmodule\n",
        "m",
    );
    assert_eq!(ty(&ports, "i"), logic(true, &[range(31, 0)], &[]));
    assert_eq!(ty(&ports, "b"), logic(false, &[range(7, 0)], &[]));
}

#[test]
fn unsupported_types() {
    let ports = ports("module m(input real r, input string s); endmodule\n", "m");
    for (name, text) in [("r", "real"), ("s", "string")] {
        let port = unsupported(&ports, name);
        assert_eq!(port.type_text, text);
        assert_eq!(port.error, "only integral (bit-vector) types are supported");
        assert!(port.loc.is_some());
    }
}

#[test]
fn struct_with_unsupported_field_is_unsupported() {
    let ports = ports(
        "typedef struct { logic a; real r; } u_t;\n\
         module m(input u_t x, input u_t y [2]); endmodule\n",
        "m",
    );
    for (name, text) in [("x", "u_t"), ("y", "u_t$[0:1]")] {
        let port = unsupported(&ports, name);
        assert_eq!(port.type_text, text);
        assert_eq!(port.error, "only integral (bit-vector) types are supported");
    }
}
