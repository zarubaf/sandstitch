// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use slang_rs::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    /// Source locations are covered by dedicated tests; these tests compare
    /// port names, directions and types only.
    fn without_locs(mut ports: HashMap<String, Vec<Port>>) -> HashMap<String, Vec<Port>> {
        for port in ports.values_mut().flatten() {
            port.loc = None;
        }
        ports
    }

    #[test]
    fn test_extract_all() {
        let verilog = str2tmpfile(
            "
            module B;
            endmodule
            module A(
              input x,
              output y
            );
              B b();
            endmodule
            package mypkg;
              localparam int myparam=42;
            endpackage
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let value = run_slang(&cfg).unwrap().ast;

        // check ports

        let ports = without_locs(extract_ports_from_value(&value, false));

        let expected_ports = HashMap::from([(
            "A".to_string(),
            vec![
                Port {
                    loc: None,
                    name: "x".to_string(),
                    dir: PortDir::Input,
                    ty: Type::Logic {
                        signed: false,
                        packed_dimensions: vec![],
                        unpacked_dimensions: vec![],
                    },
                },
                Port {
                    loc: None,
                    name: "y".to_string(),
                    dir: PortDir::Output,
                    ty: Type::Logic {
                        signed: false,
                        packed_dimensions: vec![],
                        unpacked_dimensions: vec![],
                    },
                },
            ],
        )]);

        assert_eq!(ports, expected_ports);

        // check hierarchy

        let hierarchy = extract_hierarchy_from_value(&value);

        let expected_hierarchy = HashMap::from([(
            "A".to_string(),
            Instance {
                def_name: "A".to_string(),
                inst_name: "A".to_string(),
                hier_prefix: "".to_string(),
                contents: vec![Rc::new(RefCell::new(Instance {
                    def_name: "B".to_string(),
                    inst_name: "b".to_string(),
                    hier_prefix: "".to_string(),
                    contents: vec![],
                }))],
            },
        )]);

        assert_eq!(hierarchy, expected_hierarchy);

        // check packages

        let packages = extract_packages(&cfg).unwrap();

        let expected_packages = HashMap::from([(
            "mypkg".to_string(),
            Package {
                name: "mypkg".to_string(),
                parameters: HashMap::from([(
                    "myparam".to_string(),
                    Parameter {
                        name: "myparam".to_string(),
                        value: "42".to_string(),
                        ty: None,
                    },
                )]),
            },
        )]);

        assert_eq!(packages, expected_packages);
    }
}
