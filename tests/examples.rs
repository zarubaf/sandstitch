// SPDX-License-Identifier: Apache-2.0

//! Runs the getting-started example (doc/guide.md) in CI.

#[path = "../examples/getting_started.rs"]
mod getting_started;

#[test]
fn getting_started_emits_the_design() {
    let dir = tempfile::tempdir().unwrap();
    let rtl = getting_started::run(dir.path());
    assert!(rtl.contains("module top"), "{rtl}");
    assert!(rtl.contains("adder"), "{rtl}");
    assert!(rtl.contains(".W(32'h0000_0009)"), "{rtl}");
    assert!(rtl.contains(" first ("), "{rtl}");
    assert!(rtl.contains(" second ("), "{rtl}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("top.sv")).unwrap(),
        rtl
    );
}
