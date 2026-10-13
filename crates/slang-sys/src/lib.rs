// SPDX-License-Identifier: Apache-2.0

//! Slang, built from the vendored source in `vendor/` and called in-process
//! through a narrow C++ shim (`shim/`). See
//! `doc/design/slang-in-process.md`, "Step 1".

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The vendored Slang release.
pub const SLANG_VERSION: &str = "11.0";

/// Stack of the thread each [`run`] executes on: deep nesting in a design
/// does not depend on the caller's stack.
const STACK_SIZE: usize = 64 << 20;

#[cxx::bridge(namespace = "slang_sys")]
mod ffi {
    struct FfiRequest {
        args: Vec<String>,
        /// 0: parse only; 1: elaborate.
        stage: u8,
        ast_json: bool,
        source_info: bool,
    }

    struct FfiDiag {
        /// 0 note, 1 warning, 2 error, 3 fatal.
        severity: u8,
        code: String,
        message: String,
        file: String,
        line: u32,
        column: u32,
    }

    struct FfiResponse {
        ok: bool,
        ast_json: String,
        deps: Vec<String>,
        diagnostics: Vec<FfiDiag>,
        rendered: String,
    }

    unsafe extern "C++" {
        include!("slang-sys/shim/shim.h");

        fn library_version() -> String;
        fn run(request: &FfiRequest) -> Result<FfiResponse>;
        fn trigger_library_assert() -> Result<()>;
    }
}

/// How far a [`run`] goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Process options, load and parse the sources.
    ParseOnly,
    /// Also elaborate and run analysis, as the `slang` binary does.
    Elaborate,
}

/// Whether a [`run`] serializes the AST, as `slang --ast-json` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstJson {
    None,
    /// With `source_info`, symbols carry `source_file`, `source_line` and
    /// `source_column` (`--ast-json-source-info`).
    Json {
        source_info: bool,
    },
}

/// One Slang run.
#[derive(Clone, Debug)]
pub struct Request {
    /// Options of Slang's driver and the sources, as on `slang`'s command
    /// line without the program name. Options only the `slang` binary
    /// defines (`--ast-json`, `--parse-only`, ...) are not accepted; `stage`
    /// and `ast_json` replace them.
    pub args: Vec<String>,
    pub stage: Stage,
    pub ast_json: AstJson,
}

/// The result of a [`run`].
#[derive(Clone, Debug)]
pub struct Response {
    /// No error or fatal diagnostics in the stages run.
    pub ok: bool,
    /// Empty unless requested. Also produced after errors, but not after a
    /// fatal error in option processing (a missing source, an unknown
    /// option).
    pub ast_json: String,
    /// Every file Slang loaded, included headers too: absolute,
    /// deduplicated, in load order.
    pub deps: Vec<String>,
    pub diagnostics: Vec<Diag>,
    /// The diagnostics as Slang's plain text.
    pub rendered: String,
}

/// A diagnostic's severity, least severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Note,
    Warning,
    Error,
    /// The run stopped before elaborating: a missing source, an unknown
    /// option, a bad option value.
    Fatal,
}

/// A diagnostic Slang reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag {
    pub severity: Severity,
    /// Slang's diagnostic code name, for example `UndeclaredIdentifier`;
    /// empty for errors Slang's driver printed instead of reporting.
    pub code: String,
    pub message: String,
    /// Absolute, made so lexically ([`absolute_path`]); empty for command
    /// line and global errors. The original location after macro expansion.
    pub file: String,
    /// 1-based; 0 without a file.
    pub line: u32,
    pub column: u32,
}

/// A C++ exception from Slang, including a failed internal assertion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Runs Slang on `request` in this process, on a thread of its own.
pub fn run(request: &Request) -> Result<Response, Error> {
    let mut args = request.args.clone();
    if !sets_threads(&args) {
        args.extend(["--threads".to_string(), "1".to_string()]);
    }
    let (ast_json, source_info) = match request.ast_json {
        AstJson::None => (false, false),
        AstJson::Json { source_info } => (true, source_info),
    };
    let ffi_request = ffi::FfiRequest {
        args,
        stage: match request.stage {
            Stage::ParseOnly => 0,
            Stage::Elaborate => 1,
        },
        ast_json,
        source_info,
    };
    let thread = std::thread::Builder::new()
        .name("slang".into())
        .stack_size(STACK_SIZE)
        .spawn(move || ffi::run(&ffi_request))
        .map_err(|e| Error(format!("spawning the Slang thread: {e}")))?;
    let response = thread
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        .map_err(|e| Error(e.what().to_string()))?;
    let cwd = std::env::current_dir().unwrap_or_default();
    let absolute = |path: String| {
        if path.is_empty() {
            path
        } else {
            absolute_path(&cwd, &path)
        }
    };
    let mut deps: Vec<String> = Vec::new();
    for dep in response.deps.into_iter().map(absolute) {
        if !deps.contains(&dep) {
            deps.push(dep);
        }
    }
    Ok(Response {
        ok: response.ok,
        ast_json: response.ast_json,
        deps,
        diagnostics: response
            .diagnostics
            .into_iter()
            .map(|d| Diag {
                severity: match d.severity {
                    0 => Severity::Note,
                    1 => Severity::Warning,
                    2 => Severity::Error,
                    _ => Severity::Fatal,
                },
                code: d.code,
                message: d.message,
                file: absolute(d.file),
                line: d.line,
                column: d.column,
            })
            .collect(),
        rendered: response.rendered,
    })
}

/// Whether `args` set Slang's thread count: `-j4`, `-j 4`, `--threads 4`
/// or `--threads=4`.
fn sets_threads(args: &[String]) -> bool {
    args.iter()
        .any(|a| a.starts_with("-j") || a == "--threads" || a.starts_with("--threads="))
}

/// Joins `path` onto `working_dir` and normalizes `.` and `..` lexically,
/// without touching the file system (symbolic links stay as named). An
/// absolute `path` is only normalized.
pub fn absolute_path(working_dir: &Path, path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in working_dir.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized.to_string_lossy().into_owned()
}

/// `major.minor.patch` of the linked Slang library, for example `11.0.0`.
pub fn library_version() -> String {
    ffi::library_version()
}

/// Fails an assertion inside the Slang library (`SVInt::slice` with
/// `msb < lsb`); for testing that assertions become errors.
#[doc(hidden)]
pub fn trigger_library_assert() -> Result<(), Error> {
    ffi::trigger_library_assert().map_err(|e| Error(e.what().to_string()))
}

#[cfg(test)]
#[path = "../build/support.rs"]
#[allow(dead_code)]
mod support;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_options() {
        let set =
            |args: &[&str]| sets_threads(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        assert!(set(&["-j4"]));
        assert!(set(&["-j", "4"]));
        assert!(set(&["--threads", "4"]));
        assert!(set(&["--threads=4"]));
        assert!(!set(&["a.sv", "--top", "t"]));
    }

    #[test]
    fn lexical_absolute_paths() {
        let cwd = Path::new("/work/dir");
        assert_eq!(absolute_path(cwd, "a/../b.sv"), "/work/dir/b.sv");
        assert_eq!(absolute_path(cwd, "/x/./y/../z.sv"), "/x/z.sv");
    }
}
