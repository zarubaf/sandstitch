// SPDX-License-Identifier: Apache-2.0

//! Slang, built from the vendored source in `vendor/` and called in-process
//! through a narrow C++ shim (`shim/`). See
//! `doc/design/slang-in-process.md`, "Step 1".

mod design;
mod types;
pub use design::{Design, DesignQuery};
pub use types::{
    Field, Instance, InterfacePort, Package, Parameter, ParameterDef, ParameterKind, Port, PortDir,
    Range, Signal, SourceLoc, Type, UnsupportedPort, Variant,
};

use std::fmt;
use std::path::{Component, Path, PathBuf};

/// The vendored Slang release number, the one place it is written.
macro_rules! release {
    () => {
        "12.0"
    };
}

/// The vendored Slang release.
pub const SLANG_VERSION: &str = release!();

/// The vendored Slang release, named, for example `slang 12.0`.
pub const SLANG_VERSION_LABEL: &str = concat!("slang ", release!());

/// Stack of the thread each [`run`] executes on: deep nesting in a design
/// does not depend on the caller's stack.
const STACK_SIZE: usize = 64 << 20;

#[cxx::bridge(namespace = "slang_sys")]
mod ffi {
    struct FfiRequest {
        args: Vec<String>,
        /// 0: parse only; 1: elaborate.
        stage: u8,
        /// 0: no design; 1: structure; 2: structure and signals.
        design: u8,
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
        /// Whether a compilation was created and `design` is its snapshot
        /// (the tables are empty when the request asked for no design).
        has_design: bool,
        design: FfiDesign,
        deps: Vec<String>,
        diagnostics: Vec<FfiDiag>,
        rendered: String,
    }

    // The design snapshot as flat tables. Every index is a position in the
    // table it names, as a `u32`. Strings are UTF-8 (`str()` in the shim).
    // See `design.rs` for how the tables are validated and converted.

    /// A dimension, `[msb:lsb]`.
    #[derive(Default)]
    struct FfiRange {
        msb: i32,
        lsb: i32,
    }

    /// A struct or union member.
    #[derive(Default)]
    struct FfiField {
        name: String,
        /// Index into `FfiDesign::types`, smaller than the owning type's
        /// own index. Never an unsupported type (kind 4).
        ty: u32,
    }

    /// An enum member.
    #[derive(Default)]
    struct FfiVariant {
        name: String,
        /// The value's low `width` bits, little-endian, in exactly
        /// `ceil(width / 8)` bytes; the bits above `width` in the last
        /// byte are ignored. With `is_signed`, it is two's complement over
        /// `width` bits (bit `width - 1` is the sign).
        value: Vec<u8>,
        /// Bit width of the enum's base type; at least 1.
        width: u32,
        /// Whether the base type is signed.
        is_signed: bool,
    }

    /// A source position: `line` 0 means no location; `column` 0 means no
    /// column (a macro expansion). `file` is as Slang names it; Rust makes
    /// it absolute, except pseudo-files starting with `<`.
    #[derive(Default)]
    struct FfiLoc {
        file: String,
        line: u32,
        column: u32,
    }

    /// One type, as written (see "Types" in the design document).
    ///
    /// `kind`:
    /// - 0 logic (`ScalarType`, `PredefinedIntegerType`, packed and unpacked
    ///   arrays of them): uses `is_signed`, `packed_dims`, `unpacked_dims`.
    /// - 1 struct, 2 union: `name`, `is_signed`, `packed`, `fields`, dims.
    /// - 3 enum: `name`, `variants`, dims.
    /// - 4 unsupported: only `type_text` (Slang's display of the whole
    ///   type) and `error` (the reason, one of the texts in the design
    ///   document). Everything that contains an unsupported type is itself
    ///   one entry of kind 4 rather than a struct, union or enum whose
    ///   field is unsupported; no `FfiField::ty` refers to a kind 4 entry.
    ///
    /// Dimensions are outermost first; array types contribute their range
    /// to the element type's entry. An entry refers only to entries with a
    /// smaller index (`fields[*].ty`); the table is therefore in
    /// dependency order and acyclic.
    #[derive(Default)]
    struct FfiType {
        kind: u8,
        name: String,
        is_signed: bool,
        packed: bool,
        packed_dims: Vec<FfiRange>,
        unpacked_dims: Vec<FfiRange>,
        fields: Vec<FfiField>,
        variants: Vec<FfiVariant>,
        type_text: String,
        error: String,
    }

    /// A port of a body, in declaration order.
    ///
    /// `kind`:
    /// - 0 data port: `dir` and `ty` are used. If `types[ty]` has kind 4
    ///   the port is unsupported, with that entry's `type_text` and
    ///   `error`.
    /// - 1 interface port: `interface` (empty when generic) and `modport`
    ///   (`has_modport` says whether one was written).
    /// - 2 unsupported port (a `ref` port, a multi-port): `type_text` and
    ///   `error` are used; `ty` is ignored.
    ///
    /// `dir`: 0 in, 1 out, 2 inout.
    #[derive(Default)]
    struct FfiPort {
        name: String,
        kind: u8,
        dir: u8,
        ty: u32,
        interface: String,
        modport: String,
        has_modport: bool,
        type_text: String,
        error: String,
        loc: FfiLoc,
    }

    /// A parameter of a body, in declaration order, localparams included.
    ///
    /// `kind`: 0 value parameter, 1 type parameter. For a value parameter
    /// `ty` indexes `types`; the entry may be kind 4 (unsupported), which
    /// leaves the parameter out of the typed definitions but not out of the
    /// overridable ones. `ty` is ignored for a type parameter.
    #[derive(Default)]
    struct FfiParameter {
        name: String,
        kind: u8,
        is_local: bool,
        ty: u32,
    }

    /// A member of a package, in declaration order: a value parameter or a
    /// type alias; other members are left out.
    ///
    /// `kind`: 0 value parameter, 1 type alias. `value` is the constant
    /// printed with `toString(SVInt::MAX_BITS, true, true)`
    /// or, for an alias, the alias as Slang's type printer expands it with
    /// its package-qualified name (`struct packed{logic[1:0] a;}pkg::s_t`,
    /// `logic[33:22]`). For an alias `ty` indexes `types`, the entry named as
    /// a port of that type would be and possibly kind 4 (unsupported); it is
    /// ignored for a value parameter.
    #[derive(Default)]
    struct FfiPackageParameter {
        name: String,
        kind: u8,
        value: String,
        ty: u32,
    }

    /// A package of a compilation unit.
    #[derive(Default)]
    struct FfiPackage {
        name: String,
        parameters: Vec<FfiPackageParameter>,
    }

    /// An instance inside a body.
    ///
    /// `name` is the instance name; an element of an instance array is one
    /// child named by the array and its indices as declared (`u[3]`,
    /// `u[1][0]`). `prefix` is the hierarchical prefix of the generate
    /// blocks the instance is in, empty outside generate blocks, otherwise
    /// the block names dot-joined with loop elements as `name[index]`
    /// (`g[2].h`). `definition` is the definition name.
    ///
    /// `has_body` is false for an instance of an uninstantiated or unknown
    /// definition (`UninstantiatedDefSymbol`): it has no entry in `bodies`
    /// and `body` is ignored. Otherwise `body` indexes `FfiDesign::bodies`
    /// and is smaller than the parent body's own index. Instances that
    /// share a body refer to the same entry.
    #[derive(Default)]
    struct FfiChild {
        name: String,
        definition: String,
        prefix: String,
        has_body: bool,
        body: u32,
    }

    /// A net or variable declared directly in a body, in declaration order;
    /// a port appears as its internal net or variable. `type_text` is
    /// Slang's display of the type, with type-parameter aliases seen
    /// through; `width` is `getBitstreamWidth()`.
    #[derive(Default)]
    struct FfiSignal {
        name: String,
        type_text: String,
        width: u64,
    }

    /// An instance body, numbered after every body it refers to
    /// (post-order). `signals` is empty unless the design has signals.
    #[derive(Default)]
    struct FfiBody {
        ports: Vec<FfiPort>,
        parameters: Vec<FfiParameter>,
        children: Vec<FfiChild>,
        signals: Vec<FfiSignal>,
    }

    /// A top-level instance: its name (the definition's name) and body.
    #[derive(Default)]
    struct FfiTop {
        name: String,
        body: u32,
    }

    /// The design snapshot.
    #[derive(Default)]
    struct FfiDesign {
        /// Whether the walk collected signals (`FfiRequest::design` 2).
        has_signals: bool,
        types: Vec<FfiType>,
        bodies: Vec<FfiBody>,
        tops: Vec<FfiTop>,
        packages: Vec<FfiPackage>,
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

/// One Slang run.
#[derive(Clone, Debug)]
pub struct Request {
    /// Options of Slang's driver and the sources, as on `slang`'s command
    /// line without the program name. Options only the `slang` binary
    /// defines (`--ast-json`, `--parse-only`, ...) are not accepted; `stage`
    /// replaces `--parse-only`.
    pub args: Vec<String>,
    pub stage: Stage,
    /// The design snapshot to build; only a [`Stage::Elaborate`] run builds
    /// one.
    pub design: DesignQuery,
}

/// The result of a [`run`].
#[derive(Clone, Debug)]
pub struct Response {
    /// No error or fatal diagnostics in the stages run.
    pub ok: bool,
    /// `Some` when requested and a compilation was created, also after
    /// errors; `None` when options failed or `design` was not requested.
    pub design: Option<Design>,
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
    let ffi_request = ffi::FfiRequest {
        args,
        stage: match request.stage {
            Stage::ParseOnly => 0,
            Stage::Elaborate => 1,
        },
        design: match request.design {
            DesignQuery::None => 0,
            DesignQuery::Structure => 1,
            DesignQuery::WithSignals => 2,
        },
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
    let design = if response.has_design {
        Some(design::from_ffi(response.design, &cwd)?)
    } else {
        None
    };
    let mut deps: Vec<String> = Vec::new();
    for dep in response.deps.into_iter().map(absolute) {
        if !deps.contains(&dep) {
            deps.push(dep);
        }
    }
    Ok(Response {
        ok: response.ok,
        design,
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

/// `major.minor.patch` of the linked Slang library, for example `12.0.0`.
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
