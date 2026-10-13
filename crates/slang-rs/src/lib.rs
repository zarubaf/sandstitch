// SPDX-License-Identifier: Apache-2.0

use slang_sys::{Request, Response, Stage};
use std::fmt;
use std::fs::write;
use std::path::{Path, PathBuf};

/// Slang itself, for tests and tools. Topstitch's `src/` must not use it.
pub use slang_sys as sys;
pub use slang_sys::{Design, Diag, Instance, Package, Parameter, Severity};

pub use slang_sys::{
    Field, InterfacePort, ParameterDef, ParameterKind, Port, PortDir, Range, SourceLoc, Type,
    UnsupportedPort, Variant,
};

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
    /// Also collect the nets and variables of each module body, which
    /// [`Design::signals`] returns.
    pub signals: bool,
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
            signals: false,
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

/// The result of a Slang run.
#[derive(Debug)]
pub struct SlangOutput {
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
    let response = request(cfg, Stage::Elaborate)?;
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
    let response = request(cfg, Stage::Elaborate)?;
    if response.design.is_none() {
        return Err(SlangError::from_response(response));
    }
    output(response)
}

/// Runs Slang to parse the sources only. Returns the failure as
/// [`run_slang`] does.
pub fn check_slang(cfg: &SlangConfig) -> Result<(), SlangError> {
    let response = request(cfg, Stage::ParseOnly)?;
    if response.ok {
        Ok(())
    } else {
        Err(SlangError::from_response(response))
    }
}

fn request(cfg: &SlangConfig, stage: Stage) -> Result<Response, SlangError> {
    Ok(slang_sys::run(&Request {
        args: arguments(cfg),
        stage,
        design: match stage {
            Stage::Elaborate if cfg.signals => slang_sys::DesignQuery::WithSignals,
            Stage::Elaborate => slang_sys::DesignQuery::Structure,
            Stage::ParseOnly => slang_sys::DesignQuery::None,
        },
    })?)
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

/// Turns a response of an elaborated run into the output.
fn output(mut response: Response) -> Result<SlangOutput, SlangError> {
    let Some(design) = response.design.take() else {
        return Err(SlangError::fatal(
            "Slang returned no design for an elaborated run".to_string(),
        ));
    };
    Ok(SlangOutput {
        design,
        ok: response.ok,
        deps: paths(response.deps),
        diagnostics: response.diagnostics,
    })
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
