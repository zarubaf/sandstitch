// SPDX-License-Identifier: Apache-2.0

#[cfg(test)]
mod tests {
    use slang_rs::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    #[test]
    fn test_extract_hierarchy() {
        let verilog = str2tmpfile(
            "
            module A;
              B b0();
            endmodule
            module B;
              C c0();
              C c1();
            endmodule
            module C;
            endmodule
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let hierarchy = extract_hierarchy(&cfg);

        let expected = Instance {
            def_name: "A".to_string(),
            inst_name: "A".to_string(),
            hier_prefix: "".to_string(),
            contents: vec![Rc::new(RefCell::new(Instance {
                def_name: "B".to_string(),
                inst_name: "b0".to_string(),
                hier_prefix: "".to_string(),
                contents: vec![
                    Rc::new(RefCell::new(Instance {
                        def_name: "C".to_string(),
                        inst_name: "c0".to_string(),
                        hier_prefix: "".to_string(),
                        contents: vec![],
                    })),
                    Rc::new(RefCell::new(Instance {
                        def_name: "C".to_string(),
                        inst_name: "c1".to_string(),
                        hier_prefix: "".to_string(),
                        contents: vec![],
                    })),
                ],
            }))],
        };

        let expected = HashMap::from([("A".to_string(), expected)]);

        assert_eq!(hierarchy.unwrap(), expected);
    }

    #[test]
    fn test_extract_hierarchy_genblk() {
        // Test verilog adapted from tests/unittests/ast/HierarchyTests.cpp
        // in https://github.com/MikePopoloski/slang

        let verilog = str2tmpfile(
            "
module A;
endmodule
module B;
endmodule
module top;
    parameter genblk2 = 0;
    genvar i;

    // The following generate block is implicitly named genblk1
    if (genblk2) A a(); // top.genblk1.a
    else B b(); // top.genblk1.b

    // The following generate block is implicitly named genblk02
    // as genblk2 is already a declared identifier
    if (genblk2) A a(); // top.genblk02.a
    else B b(); // top.genblk02.b

    // The following generate block would have been named genblk3
    // but is explicitly named g1
    for (i = 0; i < 1; i = i + 1) begin : g1 // block name
        // The following generate block is implicitly named genblk1
        // as the first nested scope inside g1
        if (1) A a(); // top.g1[0].genblk1.a
    end

    // The following generate block is implicitly named genblk4 since
    // it belongs to the fourth generate construct in scope 'top'.
    // The previous generate block would have been
    // named genblk3 if it had not been explicitly named g1
    for (i = 0; i < 1; i = i + 1)
        // The following generate block is implicitly named genblk1
        // as the first nested generate block in genblk4
        if (1) A a(); // top.genblk4[0].genblk1.a

    // The following generate block is implicitly named genblk5
    if (1) A a(); // top.genblk5.a
endmodule
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let hierarchy = extract_hierarchy(&cfg);

        let expected = Instance {
            def_name: "top".to_string(),
            inst_name: "top".to_string(),
            hier_prefix: "".to_string(),
            contents: vec![
                Rc::new(RefCell::new(Instance {
                    def_name: "B".to_string(),
                    inst_name: "b".to_string(),
                    hier_prefix: ".genblk1".to_string(),
                    contents: vec![],
                })),
                Rc::new(RefCell::new(Instance {
                    def_name: "B".to_string(),
                    inst_name: "b".to_string(),
                    hier_prefix: ".genblk02".to_string(),
                    contents: vec![],
                })),
                Rc::new(RefCell::new(Instance {
                    def_name: "A".to_string(),
                    inst_name: "a".to_string(),
                    hier_prefix: ".g1[0].genblk1".to_string(),
                    contents: vec![],
                })),
                Rc::new(RefCell::new(Instance {
                    def_name: "A".to_string(),
                    inst_name: "a".to_string(),
                    hier_prefix: ".genblk4[0].genblk1".to_string(),
                    contents: vec![],
                })),
                Rc::new(RefCell::new(Instance {
                    def_name: "A".to_string(),
                    inst_name: "a".to_string(),
                    hier_prefix: ".genblk5".to_string(),
                    contents: vec![],
                })),
            ],
        };

        let expected = HashMap::from([("top".to_string(), expected)]);

        assert_eq!(hierarchy.unwrap(), expected);
    }

    #[test]
    #[should_panic(expected = "error: ")]
    fn test_extract_hierarchy_error() {
        let verilog = str2tmpfile(
            "
            module A
            endmodule
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        extract_hierarchy(&cfg).unwrap();
    }

    #[test]
    fn test_unknown_module() {
        let verilog = str2tmpfile(
            "
            module E;
            endmodule
            module A(
              input clk
            );
              B b();
              if (1) begin
                C c();
              end
              if (0) begin
                D d();
              end
              E e();
            endmodule
            ",
        )
        .unwrap();

        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };

        let hierarchy = extract_hierarchy(&cfg).unwrap();

        let expected = HashMap::from([(
            "A".to_string(),
            Instance {
                def_name: "A".to_string(),
                inst_name: "A".to_string(),
                hier_prefix: "".to_string(),
                contents: vec![
                    Rc::new(RefCell::new(Instance {
                        def_name: "B".to_string(),
                        inst_name: "b".to_string(),
                        hier_prefix: "".to_string(),
                        contents: vec![],
                    })),
                    Rc::new(RefCell::new(Instance {
                        def_name: "C".to_string(),
                        inst_name: "c".to_string(),
                        hier_prefix: ".genblk1".to_string(),
                        contents: vec![],
                    })),
                    Rc::new(RefCell::new(Instance {
                        def_name: "E".to_string(),
                        inst_name: "e".to_string(),
                        hier_prefix: "".to_string(),
                        contents: vec![],
                    })),
                ],
            },
        )]);

        assert_eq!(hierarchy, expected);
    }

    /// (instance name, definition, prefix) of the children of `top`.
    fn children(source: &str, top: &str) -> Vec<(String, String, String)> {
        let verilog = str2tmpfile(source).unwrap();
        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };
        let hierarchy = extract_hierarchy(&cfg).unwrap();
        hierarchy[top]
            .contents
            .iter()
            .map(|c| {
                let c = c.borrow();
                (
                    c.inst_name.clone(),
                    c.def_name.clone(),
                    c.hier_prefix.clone(),
                )
            })
            .collect()
    }

    fn child(name: &str, def: &str, prefix: &str) -> (String, String, String) {
        (name.to_string(), def.to_string(), prefix.to_string())
    }

    #[test]
    fn genvar_prefix_uses_value() {
        let got = children(
            "module sub; endmodule
             module top;
               for (genvar i = 2; i < 4; i++) begin : g sub u(); end
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [child("u", "sub", ".g[2]"), child("u", "sub", ".g[3]")]
        );
    }

    #[test]
    fn genvar_prefix_descending_and_nested() {
        let got = children(
            "module sub; endmodule
             module top;
               for (genvar i = 5; i > 3; i--) begin : g
                 for (genvar j = 1; j < 3; j++) begin : h sub u(); end
               end
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [
                child("u", "sub", ".g[5].h[1]"),
                child("u", "sub", ".g[5].h[2]"),
                child("u", "sub", ".g[4].h[1]"),
                child("u", "sub", ".g[4].h[2]"),
            ]
        );
    }

    #[test]
    fn genvar_prefix_skips_elements_without_instances() {
        let got = children(
            "module sub; endmodule
             module top;
               for (genvar i = 2; i < 6; i++) begin : g
                 if (i % 2 == 1) begin : h sub u(); end
               end
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [child("u", "sub", ".g[3].h"), child("u", "sub", ".g[5].h")]
        );
    }

    #[test]
    fn instance_array_elements() {
        let got = children(
            "module sub; endmodule
             module top;
               sub u [3:0] ();
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [
                child("u[3]", "sub", ""),
                child("u[2]", "sub", ""),
                child("u[1]", "sub", ""),
                child("u[0]", "sub", ""),
            ]
        );
    }

    #[test]
    fn instance_array_ascending() {
        let got = children(
            "module sub; endmodule
             module top;
               sub u [0:3] ();
               sub w [2:3] ();
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [
                child("u[0]", "sub", ""),
                child("u[1]", "sub", ""),
                child("u[2]", "sub", ""),
                child("u[3]", "sub", ""),
                child("w[2]", "sub", ""),
                child("w[3]", "sub", ""),
            ]
        );
    }

    #[test]
    fn instance_array_two_dimensions() {
        let got = children(
            "module sub; endmodule
             module top;
               sub v [1:0][0:1] ();
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [
                child("v[1][0]", "sub", ""),
                child("v[1][1]", "sub", ""),
                child("v[0][0]", "sub", ""),
                child("v[0][1]", "sub", ""),
            ]
        );
    }

    #[test]
    fn instance_array_in_generate_block() {
        let got = children(
            "module sub; endmodule
             module top;
               if (1) begin : g sub u [1:0] (); end
             endmodule",
            "top",
        );
        assert_eq!(
            got,
            [child("u[1]", "sub", ".g"), child("u[0]", "sub", ".g")]
        );
    }

    #[test]
    fn undefined_module_is_a_child_without_contents() {
        let verilog =
            str2tmpfile("module top; missing m(); if (1) begin : g missing n(); end endmodule")
                .unwrap();
        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };
        let top = &extract_hierarchy(&cfg).unwrap()["top"];
        let got: Vec<_> = top
            .contents
            .iter()
            .map(|c| {
                let c = c.borrow();
                (
                    c.inst_name.clone(),
                    c.def_name.clone(),
                    c.hier_prefix.clone(),
                    c.contents.len(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("m".to_string(), "missing".to_string(), String::new(), 0),
                ("n".to_string(), "missing".to_string(), ".g".to_string(), 0),
            ]
        );
    }

    #[test]
    fn shared_body_under_two_parents() {
        let verilog = str2tmpfile(
            "module leaf; endmodule
             module mid; leaf l(); endmodule
             module top; mid a(); mid b(); endmodule",
        )
        .unwrap();
        let cfg = SlangConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ..Default::default()
        };
        let top = &extract_hierarchy(&cfg).unwrap()["top"];
        assert_eq!(top.contents.len(), 2);
        for mid in &top.contents {
            let mid = mid.borrow();
            assert_eq!(mid.def_name, "mid");
            assert_eq!(mid.contents.len(), 1);
            let leaf = mid.contents[0].borrow();
            assert_eq!(
                (leaf.def_name.as_str(), leaf.inst_name.as_str()),
                ("leaf", "l")
            );
        }
    }
}
