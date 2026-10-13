// SPDX-License-Identifier: Apache-2.0

//! The map checker accepts correct maps and rejects wrong ones with an
//! elaboration-time assertion naming the end.

use super::{expect, files};
use crate::common::slang::{Driver, Expect, Sources, check_map};

const SRC: &str = "module src(output [7:0] q); endmodule";
const SNK: &str = "module snk(input [7:0] d); endmodule";
const PSNK: &str = "module psnk #(parameter P = 1)(input [7:0] d); endmodule";

const GOOD: &str = "\
module t;
  wire [7:0] a_q;
  src a (
    .q(a_q)
  );
  snk u (
    .d({a_q[3:0], 4'h5})
  );
endmodule
";

fn good_map() -> Vec<Expect<'static>> {
    vec![
        expect("u.d", 7, 4, Driver::End("a.q", 3, 0)),
        expect("u.d", 3, 0, Driver::Const(0x5)),
        expect("a.q", 7, 4, Driver::Unused),
    ]
}

fn run(emitted: &str, module: &str, ends: &[(&str, usize)], map: &[Expect]) -> Result<(), String> {
    let sources = Sources::new(&[SRC, SNK, PSNK]);
    check_map(&files(&sources), emitted, module, ends, map)
}

/// Asserts each culprit is an elaboration-time `$error` and that every
/// error line of `err` is one.
fn assert_culprits(err: &str, culprits: &[&str]) {
    for culprit in culprits {
        let assertion = format!("error: $error encountered: {culprit}");
        assert!(err.contains(&assertion), "{culprit}: {err}");
    }
    let errors = err.lines().filter(|l| l.contains(": error: ")).count();
    let assertions = err.matches("error: $error encountered: ").count();
    assert_eq!(errors, assertions, "{err}");
}

const ENDS: [(&str, usize); 2] = [("a.q", 8), ("u.d", 8)];

#[test]
fn checker_accepts_correct_map() {
    run(GOOD, "t", &ENDS, &good_map()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn checker_rejects_swapped_bits() {
    let swapped = GOOD.replace(".d({a_q[3:0], 4'h5})", ".d({4'h5, a_q[3:0]})");
    assert_ne!(swapped, GOOD);
    let err = run(&swapped, "t", &ENDS, &good_map()).unwrap_err();
    assert_culprits(&err, &["u.d [7:4] bit 0", "u.d [3:0] const"]);
}

#[test]
fn checker_rejects_constant_wired_to_net() {
    let wired = GOOD.replace(".d({a_q[3:0], 4'h5})", ".d({a_q[3:0], a_q[7:4]})");
    assert_ne!(wired, GOOD);
    let map = [
        expect("u.d", 7, 4, Driver::End("a.q", 3, 0)),
        expect("u.d", 3, 0, Driver::Const(0x0)),
        expect("a.q", 7, 4, Driver::Unused),
    ];
    let err = run(&wired, "t", &ENDS, &map).unwrap_err();
    assert_culprits(&err, &["u.d [3:0] const", "a.q [7:4] unused bit 0"]);
    assert!(!err.contains("u.d [7:4] bit"), "{err}");
}

#[test]
fn checker_follows_assigns() {
    let text = "\
module t(
  input wire [7:0] x,
  output wire [7:0] y
);
  assign y = {x[3:0], x[7:4]};
endmodule
";
    let ends = [("x", 8), ("y", 8)];
    let map = [
        expect("y", 7, 4, Driver::End("x", 3, 0)),
        expect("y", 3, 0, Driver::End("x", 7, 4)),
    ];
    run(text, "t", &ends, &map).unwrap_or_else(|e| panic!("{e}"));

    let swapped = [
        expect("y", 7, 4, Driver::End("x", 7, 4)),
        expect("y", 3, 0, Driver::End("x", 3, 0)),
    ];
    let err = run(text, "t", &ends, &swapped).unwrap_err();
    assert_culprits(&err, &["y [7:4] bit 0", "y [3:0] bit 0"]);
}

#[test]
fn checker_rejects_read_unused() {
    let text = GOOD.replace("endmodule", "  snk w (\n    .d(a_q)\n  );\nendmodule");
    let ends = [("a.q", 8), ("u.d", 8), ("w.d", 8)];
    let mut map = good_map();
    map.push(expect("w.d", 7, 0, Driver::End("a.q", 7, 0)));
    let err = run(&text, "t", &ends, &map).unwrap_err();
    assert_culprits(&err, &["a.q [7:4] unused bit 0"]);
}

#[test]
fn checker_rejects_missing_instance_end() {
    let text = GOOD.replace("endmodule", "  snk w (\n    .d(a_q)\n  );\nendmodule");
    let err = run(&text, "t", &ENDS, &good_map()).unwrap_err();
    assert_eq!(err, "end w.d missing from ends");
}

#[test]
fn checker_rejects_missing_module_end() {
    let text = "\
module t(
  input wire [7:0] x,
  output wire [7:0] y
);
  assign y = x;
endmodule
";
    let map = [expect("x", 7, 0, Driver::Unused)];
    let err = run(text, "t", &[("x", 8)], &map).unwrap_err();
    assert_eq!(err, "end y missing from ends");
}

#[test]
fn checker_does_not_count_parameters_as_ends() {
    let text = GOOD.replace("snk u (", "psnk #(\n    .P(1)\n  ) u (");
    assert_ne!(text, GOOD);
    run(&text, "t", &ENDS, &good_map()).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn checker_rejects_mapped_twice() {
    let mut map = good_map();
    map.push(expect("u.d", 3, 0, Driver::Const(0x5)));
    let err = run(GOOD, "t", &ENDS, &map).unwrap_err();
    assert_eq!(err, "u.d bit 0 mapped twice");
}

#[test]
fn checker_rejects_uncovered_bits() {
    let map = [
        expect("u.d", 7, 4, Driver::End("a.q", 3, 0)),
        expect("u.d", 3, 0, Driver::Const(0x5)),
    ];
    let err = run(GOOD, "t", &ENDS, &map).unwrap_err();
    assert_eq!(err, "a.q bit 4 not covered by the map");
}

#[test]
fn checker_accepts_empty_connection_as_unused() {
    let text = "\
module t;
  src a (
    .q()
  );
  snk u (
    .d(8'h5)
  );
endmodule
";
    let map = [
        expect("u.d", 7, 0, Driver::Const(0x5)),
        expect("a.q", 7, 0, Driver::Unused),
    ];
    run(text, "t", &ENDS, &map).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn checker_rejects_unused_read_by_unused_end() {
    let text = "\
module t(
  input wire [7:0] en
);
  snk u (
    .d(en)
  );
endmodule
";
    let map = [
        expect("en", 7, 0, Driver::Unused),
        expect("u.d", 7, 0, Driver::Unused),
    ];
    let err = run(text, "t", &[("en", 8), ("u.d", 8)], &map).unwrap_err();
    assert_culprits(&err, &["en [7:0] unused bit 0"]);
}
