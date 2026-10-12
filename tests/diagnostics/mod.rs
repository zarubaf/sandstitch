// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;
use topstitch::diagnostic::codes;
use topstitch::{Diagnostic, Diagnostics, ImportError, Label, Severity, Span};

const RUST_FILE: &str = "tests/diagnostics/fixtures/caller.rs.txt";
const SV_FILE: &str = "tests/diagnostics/fixtures/child.sv";

fn rust_span() -> Span {
    Span::Rust {
        file: RUST_FILE.into(),
        line: 3,
        column: 5,
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

fn check_snapshot(name: &str, ds: &Diagnostics) {
    let path = PathBuf::from(format!("tests/diagnostics/snapshots/{name}.txt"));
    let actual = ds.render();
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
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

#[test]
fn span_none_and_out_of_range_do_not_panic() {
    let mut d = diag(label(Span::None, ""), vec![]);
    d.secondary.push(label(
        Span::Sv {
            file: SV_FILE.into(),
            line: 999,
            column: 1,
            len: 1,
        },
        "far away",
    ));
    let out = Diagnostics(vec![d]).render();
    assert!(out.starts_with("error[S0201]: incompatible types in connection"));
}

#[test]
fn every_code_has_index_entry() {
    for code in codes::ALL {
        let path = format!("doc/errors/{}.md", code.0);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path}"));
        assert!(text.starts_with(&format!("# {}", code.0)), "{path} title");
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
    let d = diag(label(rust_span(), "connected here"), vec![]);
    let json = Diagnostics(vec![d]).to_json();
    assert_eq!(json[0]["code"], "S0201");
    assert_eq!(json[0]["severity"], "error");
    assert_eq!(json[0]["primary"]["span"]["kind"], "rust");
    assert_eq!(json[0]["primary"]["span"]["line"], 3);
}

#[test]
fn caller_captures_location() {
    match Span::caller() {
        Span::Rust { file, line, .. } => {
            assert!(file.ends_with("mod.rs"));
            assert!(line > 0);
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
