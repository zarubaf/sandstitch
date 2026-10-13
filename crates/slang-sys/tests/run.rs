// SPDX-License-Identifier: Apache-2.0

//! `run`: stages, diagnostics, dependencies, isolation.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;

use slang_sys::{AstJson, Diag, Request, Response, Severity, Stage, run, trigger_library_assert};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> String {
    format!("{FIXTURES}/{name}")
}

fn request(args: &[&str], stage: Stage) -> Request {
    Request {
        args: args.iter().map(|a| a.to_string()).collect(),
        stage,
        ast_json: AstJson::None,
    }
}

fn elaborate(args: &[&str]) -> Response {
    run(&request(args, Stage::Elaborate)).unwrap()
}

/// A source file with `text`, deleted when dropped.
fn source(text: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".sv").tempfile().unwrap();
    file.write_all(text.as_bytes()).unwrap();
    file
}

fn path(file: &tempfile::NamedTempFile) -> &str {
    file.path().to_str().unwrap()
}

fn with_severity(r: &Response, severity: Severity) -> Vec<&Diag> {
    r.diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .collect()
}

#[test]
fn elaborates_and_lists_deps() {
    let uses = fixture("uses_header.sv");
    let inc = fixture("inc");
    let mut req = request(&["-I", &inc, &uses], Stage::Elaborate);
    req.ast_json = AstJson::Json { source_info: true };
    let r = run(&req).unwrap();
    assert!(r.ok, "{}", r.rendered);
    let ast: serde_json::Value = serde_json::from_str(&r.ast_json).unwrap();
    assert!(ast["design"]["members"].is_array());
    assert!(r.deps.contains(&uses), "{:?}", r.deps);
    assert!(r.deps.contains(&fixture("inc/defs.svh")), "{:?}", r.deps);
    let unique: HashSet<&String> = r.deps.iter().collect();
    assert_eq!(unique.len(), r.deps.len(), "{:?}", r.deps);
    assert!(
        r.deps.iter().all(|d| Path::new(d).is_absolute()),
        "{:?}",
        r.deps
    );
}

#[test]
fn parse_error_is_structured() {
    let file = source("module m(;\nendmodule\n");
    let r = elaborate(&[path(&file)]);
    assert!(!r.ok);
    let errors = with_severity(&r, Severity::Error);
    let first = errors.first().expect("an error");
    assert_eq!(first.file, path(&file));
    assert_eq!(first.line, 1);
    assert!(!first.code.is_empty());
    assert!(r.rendered.contains("error:"), "{}", r.rendered);
    assert!(!r.rendered.contains("\x1b["), "{}", r.rendered);
}

#[test]
fn macro_error_reports_original_location() {
    let r = elaborate(&[&fixture("macro_error.sv")]);
    assert!(!r.ok);
    let error = with_severity(&r, Severity::Error)[0];
    assert!(error.file.ends_with("macro_error.sv"), "{error:?}");
    // The macro body is on line 2, after the SPDX line.
    assert_eq!(error.line, 2, "{error:?}");
    assert!(error.message.contains("undeclared_name"), "{error:?}");
}

#[test]
fn missing_source_is_fatal() {
    let r = elaborate(&["/nonexistent/x.sv"]);
    assert!(!r.ok);
    let fatal = with_severity(&r, Severity::Fatal);
    assert_eq!(fatal.len(), 1, "{:?}", r.diagnostics);
    assert!(fatal[0].file.is_empty());
    assert!(fatal[0].message.contains("/nonexistent/x.sv"), "{fatal:?}");
    assert!(r.rendered.contains("/nonexistent/x.sv"), "{}", r.rendered);
    assert!(r.ast_json.is_empty());
}

#[test]
fn warning_flags_apply() {
    let width = fixture("width.sv");
    let warned = elaborate(&["-Wwidth-trunc", &width]);
    assert!(warned.ok, "{}", warned.rendered);
    assert_eq!(
        with_severity(&warned, Severity::Warning).len(),
        1,
        "{:?}",
        warned.diagnostics
    );

    let as_error = elaborate(&["-Wwidth-trunc", "-Werror", &width]);
    assert!(!as_error.ok);
    assert_eq!(
        with_severity(&as_error, Severity::Error).len(),
        1,
        "{:?}",
        as_error.diagnostics
    );

    let suppressed = elaborate(&["-Wwidth-trunc", "--suppress-warnings", &width, &width]);
    assert!(suppressed.ok);
    assert!(
        suppressed.diagnostics.is_empty(),
        "{:?}",
        suppressed.diagnostics
    );
}

#[test]
fn missing_include_dir_is_a_warning() {
    let r = elaborate(&["-I", "/nonexistent/dir", &fixture("width.sv")]);
    assert!(r.ok, "{}", r.rendered);
    let warnings = with_severity(&r, Severity::Warning);
    assert!(
        warnings
            .iter()
            .any(|d| d.message.contains("/nonexistent/dir")),
        "{:?}",
        r.diagnostics
    );
}

#[test]
fn binary_only_option_is_fatal() {
    let r = elaborate(&["--ast-json", "x.json", &fixture("width.sv")]);
    assert!(!r.ok);
    let fatal = with_severity(&r, Severity::Fatal);
    assert!(
        fatal.iter().any(|d| d.message.contains("--ast-json")),
        "{:?}",
        r.diagnostics
    );
    assert!(r.ast_json.is_empty());
}

#[test]
fn parse_only_stops_before_elaboration() {
    let file = source("module top; unknown_module u (); endmodule\n");
    let parsed = run(&request(&[path(&file)], Stage::ParseOnly)).unwrap();
    assert!(parsed.ok, "{}", parsed.rendered);
    let elaborated = elaborate(&[path(&file)]);
    assert!(!elaborated.ok);
}

/// What a test compares between runs of one request.
fn summary(r: &Response) -> (bool, Vec<(Severity, String, u32)>, Vec<String>) {
    let diags = r
        .diagnostics
        .iter()
        .map(|d| (d.severity, d.message.clone(), d.line))
        .collect();
    (r.ok, diags, r.deps.clone())
}

#[test]
fn failing_and_succeeding_calls_run_concurrently() {
    let good = fixture("width.sv");
    let requests = [vec!["/nonexistent/x.sv".to_string()], vec![good]];
    let expected: Vec<_> = requests
        .iter()
        .map(|args| summary(&run(&request_owned(args)).unwrap()))
        .collect();
    std::thread::scope(|s| {
        for t in 0..8 {
            let (requests, expected) = (&requests, &expected);
            s.spawn(move || {
                for _ in 0..10 {
                    let i = t % 2;
                    let got = summary(&run(&request_owned(&requests[i])).unwrap());
                    assert_eq!(got, expected[i], "thread {t}");
                }
            });
        }
    });
    assert!(expected[1].1.iter().all(|d| d.0 != Severity::Fatal));
}

fn request_owned(args: &[String]) -> Request {
    Request {
        args: args.to_vec(),
        stage: Stage::Elaborate,
        ast_json: AstJson::None,
    }
}

#[test]
fn relative_source_paths_become_absolute() {
    // Tests run in the crate directory.
    let r = elaborate(&[
        "-I",
        "tests/fixtures/inc",
        "tests/fixtures/../fixtures/uses_header.sv",
    ]);
    assert!(r.ok, "{}", r.rendered);
    assert!(r.deps.contains(&fixture("uses_header.sv")), "{:?}", r.deps);
    assert!(r.deps.contains(&fixture("inc/defs.svh")), "{:?}", r.deps);

    let r = elaborate(&["tests/fixtures/../fixtures/macro_error.sv"]);
    assert_eq!(r.diagnostics[0].file, fixture("macro_error.sv"));
}

/// A sum of this many terms is a left-deep expression tree, which Slang
/// binds recursively: it overflows a 2 MiB stack (calibrated by running
/// this test with the shim's own thread at 2 MiB, which crashed; 5000 terms
/// did not), while the parser handles it iteratively. Parentheses cannot
/// do this: the parser rejects them nested deeper than 1024.
const DEPTH: usize = 20_000;

#[test]
fn deep_nesting_on_a_small_stack() {
    let expr = vec!["1"; DEPTH].join(" + ");
    let file = source(&format!(
        "module deep; localparam int P = {expr}; endmodule\n"
    ));
    let args = vec![path(&file).to_string()];
    let r = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || run(&request_owned(&args)).unwrap())
        .unwrap()
        .join()
        .unwrap();
    assert!(r.ok, "{}", r.rendered);
}

#[test]
fn library_assert_is_an_error() {
    let e = trigger_library_assert().unwrap_err();
    assert!(e.to_string().contains("Assertion"), "{e}");
}
