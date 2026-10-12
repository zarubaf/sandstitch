// SPDX-License-Identifier: Apache-2.0

//! Tests for type expressions, package references and member selects.

use xlsynth_vast::{VastFile, VastFileType};

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
