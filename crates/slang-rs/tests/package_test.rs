// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use num_bigint::BigInt;
    use slang_rs::*;
    use std::collections::HashMap;

    #[test]
    fn test_extract_packages() {
        let verilog = str2tmpfile(
            "
            package pkg_a;
              localparam int a=22;
            endpackage
            package pkg_b;
              localparam int b=123;
              localparam int c=b+pkg_a::a;
              typedef logic [33:22] my_t;
            endpackage
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let pkgs = extract_packages(&cfg).unwrap();

        let file = verilog.path().to_str().unwrap().to_string();
        let expected = HashMap::from([
            (
                "pkg_a".to_string(),
                Package {
                    name: "pkg_a".to_string(),
                    file: file.clone(),
                    parameters: HashMap::from([(
                        "a".to_string(),
                        Parameter {
                            name: "a".to_string(),
                            value: "22".to_string(),
                            ty: None,
                        },
                    )]),
                },
            ),
            (
                "pkg_b".to_string(),
                Package {
                    name: "pkg_b".to_string(),
                    file: file.clone(),
                    parameters: HashMap::from([
                        (
                            "b".to_string(),
                            Parameter {
                                name: "b".to_string(),
                                value: "123".to_string(),
                                ty: None,
                            },
                        ),
                        (
                            "c".to_string(),
                            Parameter {
                                name: "c".to_string(),
                                value: "145".to_string(),
                                ty: None,
                            },
                        ),
                        (
                            "my_t".to_string(),
                            Parameter {
                                name: "my_t".to_string(),
                                value: "logic[33:22]".to_string(),
                                ty: Some(Type::Logic {
                                    signed: false,
                                    packed_dimensions: vec![Range { msb: 33, lsb: 22 }],
                                    unpacked_dimensions: vec![],
                                }),
                            },
                        ),
                    ]),
                },
            ),
        ]);

        assert_eq!(pkgs, expected);

        assert_eq!(
            pkgs["pkg_a"]["a"].parse::<BigInt>().unwrap(),
            BigInt::from(22)
        );
        assert_eq!(pkgs["pkg_b"]["b"].parse::<i32>().unwrap(), 123);
        assert_eq!(pkgs["pkg_b"]["c"].parse::<u64>().unwrap(), 145u64);
    }

    #[test]
    #[should_panic(expected = "error: expected ';'")]
    fn test_extract_packages_error() {
        let verilog = str2tmpfile(
            "
            package A
            endpackage
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        extract_packages(&cfg).unwrap();
    }

    #[test]
    fn test_extract_type_alias() {
        let verilog = str2tmpfile(
            "
            package my_pkg;
              typedef struct packed {
                logic [2:0] a;
                logic [1:0] b;
                logic c;
              } my_struct_t;
              typedef enum logic [1:0] {
                Red = 0,
                Green = 1,
                Blue = 2
              } my_enum_t;
              typedef my_struct_t [3:0] my_array_t;
            endpackage
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let pkgs = extract_packages(&cfg).unwrap();

        let ty = |name: &str| pkgs["my_pkg"][name].ty.clone().unwrap();
        let logic = |msb, lsb| Type::Logic {
            signed: false,
            packed_dimensions: vec![Range { msb, lsb }],
            unpacked_dimensions: vec![],
        };
        let my_struct = Type::Struct {
            name: "my_pkg::my_struct_t".to_string(),
            signed: false,
            packed: true,
            fields: vec![
                Field {
                    name: "a".to_string(),
                    ty: logic(2, 0),
                },
                Field {
                    name: "b".to_string(),
                    ty: logic(1, 0),
                },
                Field {
                    name: "c".to_string(),
                    ty: Type::Logic {
                        signed: false,
                        packed_dimensions: vec![],
                        unpacked_dimensions: vec![],
                    },
                },
            ],
            packed_dimensions: vec![],
            unpacked_dimensions: vec![],
        };
        assert_eq!(ty("my_struct_t"), my_struct);
        assert_eq!(
            pkgs["my_pkg"]["my_struct_t"].value,
            "struct packed{logic[2:0] a;logic[1:0] b;logic c;}my_pkg::my_struct_t"
        );

        let Type::Enum {
            name,
            variants,
            packed_dimensions,
            ..
        } = ty("my_enum_t")
        else {
            panic!("an enum alias is an enum");
        };
        assert_eq!(name, "my_pkg::my_enum_t");
        let named: Vec<_> = variants
            .iter()
            .map(|v| (v.name.as_str(), v.value.clone(), v.width))
            .collect();
        assert_eq!(
            named,
            [
                ("Red", BigInt::from(0), 2),
                ("Green", BigInt::from(1), 2),
                ("Blue", BigInt::from(2), 2)
            ]
        );
        assert!(packed_dimensions.is_empty());

        let Type::Struct {
            name,
            packed_dimensions,
            ..
        } = ty("my_array_t")
        else {
            panic!("an array of structs is a struct with a dimension");
        };
        assert_eq!(name, "my_pkg::my_struct_t");
        assert_eq!(packed_dimensions, vec![Range { msb: 3, lsb: 0 }]);
        assert_eq!(ty("my_array_t").width().unwrap(), 24);
        assert_eq!(ty("my_enum_t").width().unwrap(), 2);
        assert_eq!(my_struct.width().unwrap(), 6);
    }

    #[test]
    fn test_unsupported_alias_has_no_type() {
        let verilog = str2tmpfile(
            "
            package p;
              typedef real r_t;
              localparam int n = 1;
            endpackage
            ",
        )
        .unwrap();
        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };
        let pkgs = extract_packages(&cfg).unwrap();
        assert_eq!(pkgs["p"]["r_t"].value, "real");
        assert_eq!(pkgs["p"]["r_t"].ty, None);
        assert_eq!(pkgs["p"]["n"].ty, None);
    }
}
