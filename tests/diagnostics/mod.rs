// SPDX-License-Identifier: Apache-2.0

mod examples;
mod intf;
mod typed;

use std::path::PathBuf;
use topstitch::diagnostic::codes;
use topstitch::{Diagnostic, Diagnostics, ImportError, Label, Severity, Span};

const RUST_FILE: &str = "tests/diagnostics/fixtures/caller.rs.txt";
const SV_FILE: &str = "tests/diagnostics/fixtures/child.sv";

fn rust_span() -> Span {
    Span::Rust {
        file: RUST_FILE.into(),
        line: 4,
        column: 7,
    }
}

fn sv_span() -> Span {
    Span::Sv {
        file: SV_FILE.into(),
        line: 5,
        column: 9,
        len: 3,
    }
}

fn label(span: Span, text: &str) -> Label {
    Label {
        span,
        text: text.into(),
    }
}

fn diag(primary: Label, secondary: Vec<Label>) -> Diagnostic {
    Diagnostic {
        code: codes::S0201,
        severity: Severity::Error,
        message: "incompatible types in connection".into(),
        primary,
        secondary,
        notes: vec!["`req_t` and `req2_t` have equal width but different layouts".into()],
        helps: vec!["first differing field is `addr`".into()],
    }
}

pub(crate) fn check_snapshot(name: &str, ds: &Diagnostics) {
    let path = PathBuf::from(format!("tests/diagnostics/snapshots/{name}.txt"));
    // Snapshot files end with exactly one newline (renderer output has none).
    let actual = format!("{}\n", ds.render());
    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing snapshot {path:?}; run with UPDATE_SNAPSHOTS=1"));
    assert_eq!(actual, expected, "snapshot {name} differs");
}

#[test]
fn rust_span_snapshot() {
    let d = diag(label(rust_span(), "connected here"), vec![]);
    check_snapshot("rust_span", &Diagnostics(vec![d]));
}

#[test]
fn sv_span_snapshot() {
    let d = diag(label(sv_span(), "port declared here"), vec![]);
    check_snapshot("sv_span", &Diagnostics(vec![d]));
}

#[test]
fn both_spans_snapshot() {
    let d = diag(
        label(rust_span(), "connected here"),
        vec![label(sv_span(), "port declared here")],
    );
    check_snapshot("both_spans", &Diagnostics(vec![d]));
}

#[test]
fn missing_file_snapshot() {
    let span = Span::Sv {
        file: "tests/diagnostics/fixtures/does_not_exist.sv".into(),
        line: 5,
        column: 9,
        len: 3,
    };
    let d = diag(label(span, "port declared here"), vec![]);
    check_snapshot("missing_file", &Diagnostics(vec![d]));
}

fn missing_sv(line: u32) -> Span {
    Span::Sv {
        file: format!("tests/diagnostics/fixtures/missing{line}.sv"),
        line,
        column: 2,
        len: 3,
    }
}

#[test]
fn unresolved_secondary_does_not_become_origin() {
    let d = diag(
        label(rust_span(), "connected here"),
        vec![label(missing_sv(7), "declared here")],
    );
    check_snapshot("unresolved_secondary", &Diagnostics(vec![d]));
}

#[test]
fn primary_none_does_not_use_secondary_location() {
    let d = diag(
        label(Span::None, "no location"),
        vec![label(missing_sv(7), "declared here")],
    );
    check_snapshot("primary_none", &Diagnostics(vec![d]));
}

#[test]
fn two_unresolved_labels_keep_locations_and_texts() {
    let d = diag(
        label(missing_sv(3), "first"),
        vec![label(missing_sv(4), "second"), label(Span::None, "third")],
    );
    check_snapshot("two_missing", &Diagnostics(vec![d]));
}

#[test]
fn out_of_range_line_is_listed() {
    let span = Span::Sv {
        file: SV_FILE.into(),
        line: 999,
        column: 1,
        len: 1,
    };
    let d = diag(label(rust_span(), "here"), vec![label(span, "far away")]);
    check_snapshot("out_of_range", &Diagnostics(vec![d]));
}

#[test]
fn end_of_line_column_renders_snippet() {
    // `    a.connect(&b);` is 17 chars; column 18 is just past the end.
    let span = Span::Rust {
        file: RUST_FILE.into(),
        line: 4,
        column: 18,
    };
    check_snapshot(
        "end_of_line",
        &Diagnostics(vec![diag(label(span, "expected `;`"), vec![])]),
    );
}

#[test]
fn column_zero_is_treated_as_column_one() {
    let span = Span::Rust {
        file: RUST_FILE.into(),
        line: 4,
        column: 0,
    };
    check_snapshot(
        "column_zero",
        &Diagnostics(vec![diag(label(span, "bad column"), vec![])]),
    );
}

#[test]
fn relative_path_resolves_against_ancestor_of_cwd() {
    let pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    // Relative to the parent of the current directory, like a workspace-root-relative path.
    let span = Span::Rust {
        file: format!("{pkg}/{RUST_FILE}"),
        line: 4,
        column: 7,
    };
    let out = Diagnostics(vec![diag(label(span, "connected here"), vec![])]).render();
    assert!(out.contains("a.connect(&b);"), "{out}");
    assert!(out.contains("^^^^^^^ connected here"), "{out}");
}

#[test]
fn every_code_has_index_entry() {
    let readme = std::fs::read_to_string("doc/errors/README.md").unwrap();
    for code in codes::ALL {
        let path = format!("doc/errors/{}.md", code.0);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}"));
        assert!(text.starts_with(&format!("# {}", code.0)), "{path} title");
        assert!(text.contains("\n## Example\n"), "{path} needs ## Example");
        assert!(text.contains("\n## Fix\n"), "{path} needs ## Fix");
        assert!(
            readme.contains(&format!("| [{0}]({0}.md) |", code.0)),
            "README table lacks a row for {}",
            code.0
        );
    }
}

#[test]
fn codes_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for code in codes::ALL {
        assert!(seen.insert(code.0), "duplicate {}", code.0);
    }
}

#[test]
fn json_shape() {
    let d = diag(
        label(rust_span(), "connected here"),
        vec![
            label(sv_span(), "port declared here"),
            label(Span::None, "no span"),
        ],
    );
    let expected = serde_json::json!([{
        "code": "S0201",
        "severity": "error",
        "message": "incompatible types in connection",
        "primary": {
            "span": {"kind": "rust", "file": RUST_FILE, "line": 4, "column": 7},
            "text": "connected here"
        },
        "secondary": [
            {
                "span": {"kind": "sv", "file": SV_FILE, "line": 5, "column": 9, "len": 3},
                "text": "port declared here"
            },
            {"span": {"kind": "none"}, "text": "no span"}
        ],
        "notes": ["`req_t` and `req2_t` have equal width but different layouts"],
        "helps": ["first differing field is `addr`"]
    }]);
    assert_eq!(Diagnostics(vec![d]).to_json(), expected);
}

#[track_caller]
fn inner_helper() -> Span {
    Span::caller()
}

#[track_caller]
fn outer_helper() -> Span {
    inner_helper()
}

struct Marker;

impl Marker {
    #[track_caller]
    fn mark_here(&self) -> Span {
        Span::caller()
    }
}

#[test]
fn caller_captures_location() {
    let expected_line = line!() + 1;
    let span = outer_helper();
    match span {
        Span::Rust { file, line, .. } => {
            assert!(file.ends_with("diagnostics/mod.rs"), "{file}");
            assert_eq!(line, expected_line);
        }
        other => panic!("unexpected {other:?}"),
    }

    let expected_line = line!() + 1;
    let span = Marker.mark_here();
    let src_line = include_str!("mod.rs")
        .lines()
        .nth(expected_line as usize - 1)
        .unwrap();
    let expected_col = src_line.find("mark_here").unwrap() as u32 + 1;
    match span {
        Span::Rust { line, column, .. } => {
            assert_eq!(line, expected_line);
            assert_eq!(column, expected_col);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn has_errors_and_import_error_display() {
    let mut d = diag(label(Span::None, ""), vec![]);
    d.severity = Severity::Warning;
    assert!(!Diagnostics(vec![d.clone()]).has_errors());
    d.severity = Severity::Error;
    let ds = Diagnostics(vec![d]);
    assert!(ds.has_errors());
    let err = ImportError {
        diagnostics: ds.clone(),
    };
    assert_eq!(err.to_string(), ds.render());
    let _: &dyn std::error::Error = &err;
}

/// Checks the panic message of `f` against a snapshot, with absolute fixture
/// paths made relative.
pub(crate) fn panic_snapshot(name: &str, f: impl FnOnce()) {
    let payload =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("expected a panic");
    let message = payload.downcast::<String>().expect("a String panic");
    let root = format!("{}/", env!("CARGO_MANIFEST_DIR"));
    let actual = format!("{}\n", message.replace(&root, ""));
    let path = format!("tests/diagnostics/snapshots/{name}.txt");
    if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing snapshot {path}; run with UPDATE_SNAPSHOTS=1"));
    assert_eq!(actual, expected, "snapshot {name} differs");
}
