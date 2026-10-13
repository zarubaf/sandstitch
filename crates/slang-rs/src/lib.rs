// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use serde_json::Value;
use slang_sys::{AstJson, Request, Response, Stage};
use std::fmt;
use std::fs::write;
use std::path::{Path, PathBuf};

/// Slang itself, for tests and tools. Topstitch's `src/` must not use it.
pub use slang_sys as sys;
pub use slang_sys::{Design, Diag, Instance, Package, Parameter, Severity};

#[cfg(feature = "differential")]
mod differential;
mod extract;
pub use extract::{
    Field, InterfacePort, ParameterDef, Port, PortDir, Range, SourceLoc, Type, UnsupportedPort,
    Variant, extract_instance_interface_ports_from_value,
    extract_instance_ports_checked_from_value, extract_instance_ports_from_value,
    extract_interface_ports_from_value, extract_modules, extract_modules_from_value,
    extract_parameter_defs, extract_parameter_defs_from_value, extract_ports,
    extract_ports_from_value, extract_type_parameter_names_from_value, parse_type_definition,
};
pub use extract::{
    ParameterKind, extract_instance_parameter_defs_from_value,
    extract_overridable_parameters_from_value,
};

mod hierarchy;
pub use hierarchy::extract_hierarchy;

mod package;
pub use package::extract_packages;

#[derive(Debug)]
pub struct SlangConfig<'a> {
    pub sources: &'a [&'a str],
    pub tops: &'a [&'a str],
    pub incdirs: &'a [&'a str],
    pub defines: &'a [(&'a str, &'a str)],
    pub parameters: &'a [(&'a str, &'a str)],
    pub libfiles: &'a [&'a str],
    pub libdirs: &'a [&'a str],
    pub libexts: &'a [&'a str],
    pub ignore_unknown_modules: bool,
    pub ignore_protected: bool,
    pub timescale: Option<&'a str>,
    pub extra_arguments: &'a [&'a str],
}

impl<'a> Default for SlangConfig<'a> {
    fn default() -> Self {
        SlangConfig {
            sources: &[],
            tops: &[],
            incdirs: &[],
            defines: &[],
            parameters: &[],
            libfiles: &[],
            libdirs: &[],
            libexts: &[],
            ignore_unknown_modules: true,
            ignore_protected: true,
            timescale: None,
            extra_arguments: &[],
        }
    }
}

/// Adds options needed to make slang ignore protected envelopes.
fn push_options_to_ignore_protected(args: &mut Vec<String>) {
    let options = vec![
        "--enable-legacy-protect",
        "-Wno-protected-envelope",
        "-Wno-expected-protect-arg",
        "-Wno-expected-protect-keyword",
        "-Wno-extra-protect-end",
        "-Wno-invalid-encoding-byte",
        "-Wno-invalid-pragma-number",
        "-Wno-invalid-pragma-viewport",
        "-Wno-nested-protect-begin",
        "-Wno-protect-arglist",
        "-Wno-protect-encoding-bytes",
        "-Wno-protected-envelope",
        "-Wno-raw-protect-eof",
        "-Wno-unknown-protect-encoding",
        "-Wno-unknown-protect-keyword",
        "-Wno-unknown-protect-option",
    ];

    for option in options {
        args.push(option.to_string());
    }
}

/// Options only the `slang` binary understands. [`run_slang`] and friends
/// set the stage and the AST dump themselves; Slang's library rejects these.
pub const BINARY_ONLY_OPTIONS: [&str; 5] = [
    "--ast-json",
    "--ast-json-source-info",
    "--parse-only",
    "--disable-analysis",
    "-q",
];

/// Slang's AST as JSON. It dereferences to [`Value`]. Dropping it is
/// iterative: a plain `Value` frees nested objects recursively, which
/// overflows the stack of a small thread for a deeply nested design.
#[derive(Debug)]
pub struct Ast(Value);

impl std::ops::Deref for Ast {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.0
    }
}

impl Drop for Ast {
    fn drop(&mut self) {
        let mut stack = vec![std::mem::take(&mut self.0)];
        while let Some(value) = stack.pop() {
            match value {
                Value::Object(object) => stack.extend(object.into_iter().map(|(_, child)| child)),
                Value::Array(array) => stack.extend(array),
                _ => {}
            }
        }
    }
}

/// The result of a Slang run.
#[derive(Debug)]
pub struct SlangOutput {
    /// Slang's AST, with absolute source paths.
    pub ast: Ast,
    /// The elaborated design: instances and their ports.
    pub design: Design,
    /// No error or fatal diagnostics.
    pub ok: bool,
    /// Every file Slang loaded, headers included; absolute.
    pub deps: Vec<PathBuf>,
    /// Slang's diagnostics of every severity, in order.
    pub diagnostics: Vec<Diag>,
}

/// A failed Slang run.
#[derive(Debug)]
pub struct SlangError {
    pub diagnostics: Vec<Diag>,
    /// The diagnostics as Slang's plain text.
    pub rendered: String,
    pub deps: Vec<PathBuf>,
}

impl fmt::Display for SlangError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.rendered.is_empty() {
            f.write_str(&self.rendered)
        } else if let Some(first) = self.diagnostics.first() {
            f.write_str(&first.message)
        } else {
            f.write_str("slang failed")
        }
    }
}

impl std::error::Error for SlangError {}

impl SlangError {
    fn fatal(message: String) -> Self {
        SlangError {
            diagnostics: vec![Diag {
                severity: Severity::Fatal,
                code: String::new(),
                message: message.clone(),
                file: String::new(),
                line: 0,
                column: 0,
            }],
            rendered: message,
            deps: Vec::new(),
        }
    }

    fn from_response(response: Response) -> Self {
        SlangError {
            diagnostics: response.diagnostics,
            rendered: response.rendered,
            deps: paths(response.deps),
        }
    }
}

impl From<slang_sys::Error> for SlangError {
    fn from(error: slang_sys::Error) -> Self {
        SlangError::fatal(error.0)
    }
}

fn paths(deps: Vec<String>) -> Vec<PathBuf> {
    deps.into_iter().map(PathBuf::from).collect()
}

/// The Slang library [`run_slang`] calls, for example `slang 11.0`.
pub fn slang_version() -> String {
    format!("slang {}", slang_sys::SLANG_VERSION)
}

/// Runs Slang; an error unless the design elaborated without errors.
pub fn run_slang(cfg: &SlangConfig) -> Result<SlangOutput, SlangError> {
    let response = request(cfg, Stage::Elaborate, source_info())?;
    if !response.ok {
        return Err(SlangError::from_response(response));
    }
    output(response)
}

/// Like [`run_slang`], but a run that fails still returns the design, with
/// `ok` false, whenever Slang created a compilation. Every declared member
/// is kept; a port whose type Slang could not resolve is unsupported. After
/// a syntax error the design is Slang's recovery and may not match the
/// source. Returns the failure only if Slang created no compilation (a
/// missing source, an unknown option).
pub fn run_slang_keep_design(cfg: &SlangConfig) -> Result<SlangOutput, SlangError> {
    let response = request(cfg, Stage::Elaborate, source_info())?;
    if response.design.is_none() {
        return Err(SlangError::from_response(response));
    }
    output(response)
}

/// Runs Slang to parse the sources only, without an AST. Returns the failure
/// as [`run_slang`] does.
pub fn check_slang(cfg: &SlangConfig) -> Result<(), SlangError> {
    let response = request(cfg, Stage::ParseOnly, AstJson::None)?;
    if response.ok {
        Ok(())
    } else {
        Err(SlangError::from_response(response))
    }
}

/// Source info adds `source_file`/`source_line`/`source_column` to each
/// symbol, which is used to fill in declaration locations (e.g. `Port::loc`).
fn source_info() -> AstJson {
    AstJson::Json { source_info: true }
}

fn request(cfg: &SlangConfig, stage: Stage, ast_json: AstJson) -> Result<Response, SlangError> {
    let response = slang_sys::run(&Request {
        args: arguments(cfg),
        stage,
        ast_json,
        design: match stage {
            Stage::Elaborate => slang_sys::DesignQuery::Structure,
            Stage::ParseOnly => slang_sys::DesignQuery::None,
        },
    })?;
    #[cfg(feature = "differential")]
    differential::check_agreement(&response);
    Ok(response)
}

fn arguments(cfg: &SlangConfig) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if cfg.ignore_unknown_modules {
        args.push("--ignore-unknown-modules".to_string());
    }

    if cfg.ignore_protected {
        push_options_to_ignore_protected(&mut args);
    }

    args.extend(cfg.extra_arguments.iter().map(|a| a.to_string()));

    for (name, value) in cfg.parameters.iter() {
        args.push("-G".to_string());
        args.push(format!("{name}={value}"));
    }

    for top in cfg.tops.iter() {
        args.push("--top".to_string());
        args.push(top.to_string());
    }

    for incdir in cfg.incdirs.iter() {
        args.push("-I".to_string());
        args.push(incdir.to_string());
    }

    for (name, value) in cfg.defines.iter() {
        args.push("-D".to_string());
        args.push(format!("{name}={value}"));
    }

    for libfile in cfg.libfiles.iter() {
        args.push("-v".to_string());
        args.push(libfile.to_string());
    }

    for libdir in cfg.libdirs.iter() {
        args.push("-y".to_string());
        args.push(libdir.to_string());
    }

    for libext in cfg.libexts.iter() {
        args.push("-Y".to_string());
        args.push(libext.to_string());
    }

    if let Some(timescale) = cfg.timescale {
        args.push("--timescale".to_string());
        args.push(timescale.to_string());
    }

    args.extend(cfg.sources.iter().map(|s| s.to_string()));
    args
}

/// Turns a response that carries an AST and a design into the output.
fn output(mut response: Response) -> Result<SlangOutput, SlangError> {
    let Some(design) = response.design.take() else {
        return Err(SlangError::fatal(
            "Slang returned no design for an elaborated run".to_string(),
        ));
    };
    // Slang reports source paths relative to its working directory, which is
    // this process's; capture it so they can be made absolute.
    let ast = std::env::current_dir()
        .map_err(|e| e.to_string())
        .and_then(|dir| parse_ast(&response.ast_json, &dir).map_err(|e| e.to_string()));
    match ast {
        Ok(ast) => {
            #[cfg(feature = "differential")]
            differential::check(&ast, &design);
            Ok(SlangOutput {
                ast,
                design,
                ok: response.ok,
                deps: paths(response.deps),
                diagnostics: response.diagnostics,
            })
        }
        Err(message) => Err(SlangError::fatal(format!("reading Slang's AST: {message}"))),
    }
}

/// Parses the AST Slang serialized.
fn parse_ast(json: &str, working_dir: &Path) -> Result<Ast, serde_json::Error> {
    let mut json_deserializer = serde_json::Deserializer::from_str(json);
    // The recursion limit needs to be disabled since slang can produce
    // deeply nested objects.
    // Follows the example code here:
    // https://docs.rs/serde_json/latest/serde_json/struct.Deserializer.html#method.disable_recursion_limit
    json_deserializer.disable_recursion_limit();

    let stacked_deserializer = serde_stacker::Deserializer::new(&mut json_deserializer);
    let mut ast = Ast(Value::deserialize(stacked_deserializer)?);
    absolutize_source_files(&mut ast.0, working_dir);

    Ok(ast)
}

/// Rewrites the `source_file*` fields added by `--ast-json-source-info`
/// (relative to Slang's working directory) to absolute, lexically normalized
/// paths. Pseudo-files such as `<command-line>` are left unchanged.
fn absolutize_source_files(value: &mut Value, working_dir: &Path) {
    // Iterative: the AST can be nested deeper than the native stack allows.
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                for (key, child) in object.iter_mut() {
                    if key.starts_with("source_file") {
                        if let Value::String(path) = child {
                            if !path.starts_with('<') {
                                *path = absolute_path(working_dir, path);
                            }
                        }
                    } else {
                        stack.push(child);
                    }
                }
            }
            Value::Array(array) => stack.extend(array.iter_mut()),
            _ => {}
        }
    }
}

/// Joins `path` onto `working_dir` and normalizes `.` and `..` lexically
/// (without touching the file system). An absolute `path` is kept as is.
pub fn absolute_path(working_dir: &Path, path: &str) -> String {
    slang_sys::absolute_path(working_dir, path)
}

pub fn str2tmpfile(s: &str) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
    let tmp_file = tempfile::NamedTempFile::new()?;
    write(tmp_file.path(), s)?;
    Ok(tmp_file)
}
