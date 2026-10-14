// SPDX-License-Identifier: Apache-2.0

//! Tests for type expressions, package references and member selects.

use xlsynth_vast::{DataKind, Expr, VastFile, VastFileType};

#[test]
fn instantiation_with_type_and_package_constant_overrides() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let module = file.add_module("top");
    let flit = file.make_extern_package_type("cfg_pkg", "flit_t");
    let array = file.make_packed_array_type(flit, &[5]);
    let n = file.add_wire(module, "n", &array);
    let sel = file.make_index(&n.to_indexable_expr(), 2);
    let hdr = file.make_member_select(&sel.to_expr(), "hdr");
    let valid = file.make_member_select(&hdr, "valid");
    let t = file.make_type_expr(&flit);
    let c = file.make_package_ref("cfg_pkg", "Cfg");
    let inst = file.make_instantiation(
        "child",
        "u_child",
        &["T", "C"],
        &[&t, &c],
        &["x"],
        &[Some(&valid)],
    );
    file.add_member_instantiation(module, inst);
    assert_eq!(
        file.emit(),
        "module top;\n  wire cfg_pkg::flit_t [4:0] n;\n  child #(\n    .T(cfg_pkg::flit_t),\n    .C(cfg_pkg::Cfg)\n  ) u_child (\n    .x(n[2].hdr.valid)\n  );\nendmodule\n"
    );
}

#[test]
#[should_panic(expected = "unpacked array types cannot be used as type expressions")]
fn type_expr_rejects_unpacked_arrays() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let byte = file.make_bit_vector_type(8, false);
    let unpacked = file.make_unpacked_array_type(byte, &[2]);
    file.make_type_expr(&unpacked);
}

#[test]
#[should_panic(expected = "member selects require a name")]
fn member_select_rejects_non_name_base() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let module = file.add_module("top");
    let bit = file.make_scalar_type();
    let w = file.add_wire(module, "w", &bit);
    let a = w.to_expr();
    let sum = file.make_add(&a, &a);
    file.make_member_select(&sum, "f");
}

#[test]
#[should_panic(expected = "member select field must be a simple identifier")]
fn member_select_rejects_invalid_field() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let module = file.add_module("top");
    let bit = file.make_scalar_type();
    let w = file.add_wire(module, "w", &bit);
    file.make_member_select(&w.to_expr(), "a b");
}

#[test]
#[should_panic(expected = "package name must be a simple identifier")]
fn package_ref_rejects_empty_package() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    file.make_package_ref("", "X");
}

#[test]
fn type_expr_spells_builtin_and_packed_types() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let bits = file.make_bit_vector_type(8, false);
    let expr = file.make_type_expr(&bits);
    assert_eq!(file.emit_expression(&expr), "logic [7:0]");
    let scalar = file.make_scalar_type();
    let expr = file.make_type_expr(&scalar);
    assert_eq!(file.emit_expression(&expr), "logic");
    let flit = file.make_extern_package_type("cfg_pkg", "flit_t");
    let packed = file.make_packed_array_type(flit, &[5]);
    let expr = file.make_type_expr(&packed);
    assert_eq!(file.emit_expression(&expr), "cfg_pkg::flit_t [4:0]");
}

#[test]
fn member_select_chains_indexes_and_assigns() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let module = file.add_module("top");
    let flit = file.make_extern_package_type("cfg_pkg", "flit_t");
    let array = file.make_packed_array_type(flit, &[5]);
    let n = file.add_wire(module, "n", &array);
    let sel = file.make_index(&n.to_indexable_expr(), 2);
    let hdr = file.make_member_select(&sel.to_expr(), "hdr");
    let valid = file.make_member_select(&hdr, "valid");
    assert_eq!(file.emit_expression(&valid), "n[2].hdr.valid");

    let bits = file.make_member_select_indexable(&sel.to_expr(), "bits");
    let bit = file.make_index(&bits, 3);
    assert_eq!(file.emit_expression(&bit.to_expr()), "n[2].bits[3]");
    let slice = file.make_slice(&bits, 3, 1);
    assert_eq!(file.emit_expression(&slice.to_expr()), "n[2].bits[3:1]");

    let m = file.add_wire(module, "m", &flit);
    let lhs = file.make_member_select(&m.to_expr(), "valid");
    let assign = file.make_continuous_assignment(&lhs, &valid);
    file.add_member_continuous_assignment(module, assign);
    let text = file.emit();
    assert!(text.contains("wire cfg_pkg::flit_t [4:0] n;"), "{text}");
    assert!(text.contains("assign m.valid = n[2].hdr.valid;"), "{text}");
}

#[test]
fn name_ref_spells_a_bare_identifier() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let expr = file.make_name_ref("Depth$1");
    assert_eq!(file.emit_expression(&expr), "Depth$1");
}

#[test]
#[should_panic(expected = "name reference must be a simple identifier")]
fn name_ref_rejects_scoped_names() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    file.make_name_ref("pkg::X");
}

#[test]
fn packed_array_with_declared_ranges() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let module = file.add_module("top");
    let flit = file.make_extern_package_type("cfg_pkg", "flit_t");
    let array = file.make_packed_array_type_ranges(flit, &[(0, 2), (5, 4)]);
    let p = file.add_output(module, "p", &array);
    let bits = file.make_bit_vector_type(8, false);
    let vector = file.make_packed_array_type_ranges(bits, &[(3, 1)]);
    // Sizes follow the ranges.
    assert_eq!(file.type_flat_bit_count_as_int64(vector).unwrap(), 24);
    file.add_wire(module, "v", &vector);
    // Indices are the declared ones.
    let element = file.make_index(&p.to_indexable_expr(), 5);
    let data = file.make_member_select(&element.to_expr(), "data");
    let w = file.add_wire(module, "w", &bits);
    let assignment = file.make_continuous_assignment(&w.to_expr(), &data);
    file.add_member_continuous_assignment(module, assignment);
    assert_eq!(
        file.emit(),
        "module top(\n  output cfg_pkg::flit_t [0:2][5:4] p\n);\n  wire [3:1][7:0] v;\n  wire [7:0] w;\n  assign w = p[5].data;\nendmodule\n"
    );
}

#[test]
#[should_panic(expected = "packed-array range bounds must not be negative")]
fn packed_array_ranges_reject_negative_bounds() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let bits = file.make_bit_vector_type(8, false);
    file.make_packed_array_type_ranges(bits, &[(1, -1)]);
}

fn typedef_hdr_macro_args(file: &mut VastFile) -> Vec<Expr> {
    vec![
        file.make_name_ref("hdr_t"),
        file.make_name_ref("xy_t"),
        file.make_name_ref("logic"),
        file.make_unsized_decimal_literal(2),
    ]
}

#[test]
fn package_with_every_member_kind() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    file.add_include("ext/typedef.svh");
    let blank = file.make_blank_line();
    file.add_blank_line(blank);
    let package = file.add_package("cfg_pkg");

    let cfg_t = file.make_extern_package_type("ext_pkg", "cfg_t");
    let cfg_def = file.make_def("Cfg", DataKind::User, &cfg_t);
    let width = file.make_unsized_decimal_literal(48);
    let depth = file.make_unsized_decimal_literal(4);
    let cfg = file.make_named_assignment_pattern(&[("Width", width), ("Depth", depth)]);
    file.add_package_localparam(package, &cfg_def, &cfg);

    let uint = file.make_int_type(false);
    let count_def = file.make_def("Count", DataKind::Int, &uint);
    let one = file.make_unsized_decimal_literal(1);
    file.add_package_localparam(package, &count_def, &one);

    let bits2 = file.make_bit_vector_type(2, false);
    file.add_package_typedef(package, "x_bits_t", &bits2);
    file.add_package_struct_typedef(package, "xy_t", &[("x", bits2), ("y", bits2)]);
    let zero = file.make_unsized_decimal_literal(0);
    let one = file.make_unsized_decimal_literal(1);
    file.add_package_enum_typedef(package, "dir_e", &bits2, &[("A", zero), ("B", one)]);

    let xy_t = file.make_extern_type("xy_t");
    let origin_def = file.make_def("Origin", DataKind::User, &xy_t);
    let x = file.make_unsized_decimal_literal(1);
    let y = file.make_unsized_decimal_literal(0);
    let origin = file.make_named_assignment_pattern(&[("x", x), ("y", y)]);
    file.add_package_localparam(package, &origin_def, &origin);

    let dir_e = file.make_extern_type("dir_e");
    let start_def = file.make_def("Start", DataKind::User, &dir_e);
    let b = file.make_name_ref("B");
    file.add_package_localparam(package, &start_def, &b);

    let args = typedef_hdr_macro_args(&mut file);
    let macro_ref =
        file.make_macro_ref_with_args("TYPEDEF_HDR_T", &args.iter().collect::<Vec<_>>());
    let statement = file.make_macro_statement(&macro_ref, false);
    file.add_package_macro(package, statement);

    let expected = concat!(
        "`include \"ext/typedef.svh\"\n",
        "\n",
        "package cfg_pkg;\n",
        "  localparam ext_pkg::cfg_t Cfg = '{Width: 48, Depth: 4};\n",
        "  localparam int unsigned Count = 1;\n",
        "  typedef logic [1:0] x_bits_t;\n",
        "  typedef struct packed {\n",
        "    logic [1:0] x;\n",
        "    logic [1:0] y;\n",
        "  } xy_t;\n",
        "  typedef enum logic [1:0] {\n",
        "    A = 0,\n",
        "    B = 1\n",
        "  } dir_e;\n",
        "  localparam xy_t Origin = '{x: 1, y: 0};\n",
        "  localparam dir_e Start = B;\n",
        "  `TYPEDEF_HDR_T(hdr_t, xy_t, logic, 2)\n",
        "endpackage\n",
    );
    assert_eq!(file.emit(), expected);
}

#[test]
fn macro_in_package_has_no_semicolon() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let package = file.add_package("p");
    let args = typedef_hdr_macro_args(&mut file);
    let macro_ref =
        file.make_macro_ref_with_args("TYPEDEF_HDR_T", &args.iter().collect::<Vec<_>>());
    let statement = file.make_macro_statement(&macro_ref, false);
    file.add_package_macro(package, statement);
    assert!(
        file.emit()
            .contains("  `TYPEDEF_HDR_T(hdr_t, xy_t, logic, 2)\nendpackage")
    );
}

#[test]
fn one_bit_typedef_has_a_range() {
    let mut file = VastFile::new(VastFileType::SystemVerilog);
    let package = file.add_package("p");
    let bit = file.make_scalar_type();
    let ranged = file.make_packed_array_type_ranges(bit, &[(0, 0)]);
    file.add_package_typedef(package, "b_t", &ranged);
    assert!(file.emit().contains("  typedef logic [0:0] b_t;\n"));
}
