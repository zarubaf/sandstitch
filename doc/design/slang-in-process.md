# Slang in-process

Status: accepted, 2026-10-13; step 1 implemented (#17). Tracks #5; step 1 is #17, step 2 is #18.

## Goal

Link Slang into the library instead of running a `slang` executable and parsing its `--ast-json` output. Sandstitch is used as a crate in downstream RTL projects, and later through Python bindings; it is not shipped as a prebuilt command line tool. The question this answers is how Slang ships with the crate.

Before step 1 (#17), `crates/slang-rs` found a binary through `SLANG_PATH` or `PATH`. A specialization ran it up to four times: the module definition, a parse check, the elaboration of a generated wrapper top, and a second parse check; the definition and elaboration runs also wrote a `--Mall` dependency file. Each run wrote the AST to a temporary JSON file (about 37 MB for one FlooNoC router), from which ports, types, parameters and packages were reconstructed out of about 15 AST node kinds. Type structure arrives as text (`<addr> <name>` links, `$` unpacked separators, sized enum literals) and is parsed back with a regex resolver and a pest grammar; this remains until step 2 (#18). Errors were scraped from stderr. Correctness depended on which `slang` was first on the user's `PATH`, because the JSON format changes between Slang releases.

## Decisions

1. **Link, don't spawn.** Slang is compiled from pinned vendored source and linked statically. The crate does not use any `slang` the user has installed; a project's own Slang for linting or simulation is independent, as with any other frontend.
1. **Three layers.** `topstitch` import (model concepts) → `slang-rs` (safe typed Rust API, no model concepts) → `slang-sys` (vendored C++, build, `cxx` shim). Slang details stay out of the model and out of the library boundary; `topstitch` does not depend on `slang-sys` directly.
1. **A narrow shim, not general bindings.** The shim answers the importer's questions and returns plain data. Binding Slang's object graph to Rust is out of scope.
1. **Two steps.** Step 1 moves Slang in-process and keeps the JSON extractors, so every existing test checks the swap. Step 2 replaces the extractors with typed queries, checked against the JSON path until it is removed.
1. **`slang-rs` diverges from upstream.** xlsynth's `slang-rs` stays subprocess and JSON based. Its fixes are ported by hand; `doc/fork.md` records this.
1. **One way to build.** The crate always builds its vendored Slang. Linking a prebuilt library is left out until someone needs it: a foreign build can differ in allocator, Boost, position independence and assertion settings in ways that cannot be checked reliably at link time.

## Spike

A throwaway spike (Linux x86-64, GCC 14, 16 cores shared with other load) built Slang v11.0 as a static library and called it through a 60-line `cxx` shim:

- The shim elaborates `floo_synth_nw_router`; the router's link ports are 5 × 116, 100 and 1204 bits, as the JSON import reports.
- A cold build of `libsvlang.a` takes about 4 minutes at `-j16` and 6 at `-j4`. A warm ccache build in a fresh build directory takes 1 second.
- CMake 4 scans C++20 sources for modules by default (`-fmodules-ts`), which ccache cannot cache; `CMAKE_CXX_SCAN_FOR_MODULES=OFF` fixes it. ccache only hits across build directories when paths are rewritten relative to a common base directory.
- The Slang source tarball is 1.9 MB; `libsvlang.a` is 27 MB; the test binary is 9.9 MB.

## Build (`crates/slang-sys`)

**Sources.** Slang v11.0 and fmt 12.1.0, unpacked under `crates/slang-sys/vendor/`, with their license files. Of Slang, `include/`, `source/`, `external/`, `cmake/`, `scripts/` (the diagnostic and syntax generators run at build time) and the CMake files are kept; tests, docs, `pyslang/`, `tools/` and `bindings/` are dropped. `doc/fork.md` records the versions and the update procedure.

**CMake.** `build.rs` copies the vendored sources into `OUT_DIR`, only when they differ from the existing copy and keeping their timestamps (a needless copy would reconfigure and rebuild Slang), and builds there with the `cmake` crate, with `no_default_flags(true)` so the Cargo profile does not inject `-g` or optimization flags:

- `CMAKE_BUILD_TYPE=Release` for every Cargo profile.
- `SLANG_INCLUDE_TOOLS`, `SLANG_INCLUDE_TESTS`, `SLANG_INCLUDE_INSTALL` and `SLANG_USE_MIMALLOC` off; `CMAKE_POSITION_INDEPENDENT_CODE=ON` (Python extension modules later).
- `SLANG_ASSERT_ENABLED=1` for the library and the shim alike: a failed internal assertion throws `AssertionException`, which reaches Rust as an error instead of undefined behavior.
- `CMAKE_CXX_SCAN_FOR_MODULES=OFF`.
- Vendored dependencies only: `FETCHCONTENT_SOURCE_DIR_FMT` pointing to the copied fmt, `FETCHCONTENT_FULLY_DISCONNECTED=ON` and `FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER` (no network, no system fmt), and `CMAKE_DISABLE_FIND_PACKAGE_Boost=ON` (always the vendored single-header `boost_unordered`, matching the shim's `SLANG_BOOST_SINGLE_HEADER`).

The shim is compiled with `cxx-build` against the copied headers and the generated ones in the CMake build directory (`slang_export.h`, syntax and diagnostic headers), with `SLANG_STATIC_DEFINE`, `SLANG_BOOST_SINGLE_HEADER`, `SLANG_USE_THREADS` and `SLANG_ASSERT_ENABLED=1`, and links `svlang`, `fmt` and the C++ standard library. A test (`crates/slang-sys/tests/version.rs`) checks that the linked library's `VersionInfo` reports `SLANG_VERSION`; the git hash in Slang's version string is `0`, since the vendored copy has no Slang history, which is expected.

**Compiler cache.** If `ccache` is on `PATH`, `build.rs` sets it as the C and C++ compiler launcher with `CCACHE_BASEDIR` set to `OUT_DIR` and `CCACHE_NOHASHDIR=1`. Sources, generated headers and objects all live under `OUT_DIR`, so every path is relative to it and the cache hits across profiles, targets, `cargo clean` and other projects using the same version. `SLANG_SYS_NO_CCACHE=1` disables this. sccache is not configured: it does not rewrite paths, so it would not hit across `OUT_DIR`s.

**Requirements.** C++20 with GCC 11 or Clang 17 or newer, CMake 3.20 or newer, and Python 3 (Slang generates sources at build time). `build.rs` checks all three before configuring and fails with a message naming the requirement and what it found; for CMake it suggests `pip install cmake`, which needs no root. The C++ standard library is linked dynamically, so a binary built with GCC 11 needs at least that libstdc++ at run time. Linux x86-64 is tested; other targets are not blocked, but are documented as untested.

**CI.** `.github/workflows/ci.yml` installs ccache and caches its directory and Cargo's target directory, so ordinary runs do not rebuild Slang. No separate native build job.

## Step 1: Slang in-process (#17)

### Shim

One C++ file and a `cxx` bridge in `slang-sys`:

```rust
pub struct Request {
    pub args: Vec<String>,   // Driver options: sources, -I, -D, -G, --top, -y, ... (no argv[0])
    pub stage: Stage,        // ParseOnly | Elaborate
    pub ast_json: AstJson,   // None | Json { source_info: bool }
}

pub struct Response {
    pub ok: bool,                // no error diagnostics in the stages run
    pub ast_json: String,        // empty unless requested; also produced when !ok
    pub deps: Vec<String>,       // loaded files: absolute, deduplicated, no macro buffers
    pub diagnostics: Vec<Diag>,  // all diagnostics, including option and file errors
    pub rendered: String,        // the same diagnostics as Slang's plain text
}

pub struct Diag {
    pub severity: Severity,      // Note | Warning | Error | Fatal
    pub code: String,            // Slang's diagnostic code name, e.g. "UndeclaredIdentifier"
    pub message: String,         // Slang's message text
    pub file: String,            // absolute; empty for command line and global errors
    pub line: u32,               // 1-based; of the original location after macro expansion
    pub column: u32,
}

pub fn run(req: &Request) -> Result<Response, Error>;  // Error: the exception's what() text
pub const SLANG_VERSION: &str = "11.0";                // the vendored release
```

**Options.** `args` holds only options registered by `driver::Driver`, parsed by `Driver::parseCommandLine` as the `slang` binary parses them. Options that only the binary's `main` defines (`--ast-json`, `--ast-json-source-info`, `--parse-only`, `--disable-analysis`, `-q`) are not accepted in `args`; `stage` and `ast_json` replace them. Unless `args` sets the thread count in any form (`-j4`, `-j 4`, `--threads 4`, `--threads=4`), the shim adds `--threads 1`, so concurrent calls do not each start a thread pool.

**Stages,** in the order `slang` runs them:

1. Process options and load and parse all sources. Option, file and parse errors are recorded. With `Stage::ParseOnly`, stop here.
1. Create the compilation and elaborate.
1. Run analysis, as the binary does without `--disable-analysis`.
1. If requested, serialize the AST with Slang's `ASTSerializer` and `JsonWriter` (the code behind `--ast-json`), including after errors, so that `run_slang_keep_ast` keeps working.

**Diagnostics.** `Driver` prints some problems directly instead of reporting them as diagnostics: unknown options, missing source files, a bad `--timescale` and unreadable `-f` files as errors; a missing include directory or `--suppress-warnings` path as warnings; and parse progress to stdout. These are captured: constructing the `Driver`, turning colors off, option processing and stage 1 run under one process-wide mutex with `OS::captureOutput` on, and both captured streams are drained before the mutex is released. The captured stderr is split at the `error: `, `warning: ` and `note: ` prefixes `Driver` writes; errors become `Fatal` diagnostics without a file, warnings and notes keep their severity, so an import that the binary accepts with a warning still succeeds. Diagnostics reach the shim through Slang's diagnostic engine, so `-W…`, `-Werror`, `--suppress-warnings`, pragmas and the error limit apply: the shim replaces the driver's text client with a recording client and an uncolored text client of its own, issues the syntax trees' diagnostics itself for `ParseOnly`, and the compilation's (which include the parse diagnostics) for `Elaborate`. Command line errors become `Fatal`. A source that fails to load is `Fatal` but, as in `slang`, the run elaborates the rest; only failed option processing skips elaboration. Elaboration, analysis and serialization run outside the mutex, in parallel; this is sound only because nothing after stage 1 prints, so the shim never calls Slang's reporting functions (`reportParseDiags`, `reportCompilation`, `reportDiagnostics`, `runFullCompilation`, `optionallyWriteDepFiles`), whose output would land in another call's capture. The mutex serializes parsing across concurrent imports; the performance measurements record its cost. Locations are mapped to where the text is written (inside a macro, its definition; `rendered` keeps Slang's use-site context), and file names are made absolute lexically by the Rust side of `slang-sys` (`absolute_path`, without resolving symbolic links), the same function `slang-rs` applies to AST source paths, so all three agree. Options whose effect lives only in those functions (`--Mall`, `--Minclude`, `--Mmodule`, `--depfile-target`, `--diag-json`, in either spelling) would silently do nothing, so the shim reports each as `Fatal` after option processing and skips elaboration. All text fields cross the boundary with `rust::String::lossy`, so invalid UTF-8 in a diagnostic becomes U+FFFD instead of failing the run. `rendered` uses Slang's text client with colors forced off; the shim never writes to the process's stdout or stderr.

**Isolation.** Each call runs on its own thread with a 64 MiB stack, joined before `run` returns, so deep nesting does not depend on the caller's stack size (2 MiB for test and spawned threads). Each call builds its own `Driver` and `Compilation`; after constructing the `Driver` the shim turns terminal colors off, since the constructor sets them from whether stderr is a terminal. A C++ exception, including a failed internal assertion, becomes `Err`, except where Slang asserts inside a `noexcept` function or destructor, which terminates. A crash in Slang that is not an exception still ends the host process; this is the cost of running in-process.

### `slang-rs`

The run functions return what the caller needs instead of a bare `Value`:

```rust
pub struct SlangOutput { pub ast: Ast, pub ok: bool, pub deps: Vec<PathBuf> }
pub struct SlangError  { pub diagnostics: Vec<Diag>, pub rendered: String, pub deps: Vec<PathBuf> }

pub fn run_slang(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>;           // ok is true
pub fn run_slang_keep_ast(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>; // ok may be false
pub fn check_slang(cfg: &SlangConfig) -> Result<(), SlangError>;                 // Stage::ParseOnly
```

`Ast` is a newtype over `serde_json::Value` that dereferences to it and drops iteratively, so deep ASTs do not overflow small stacks; it derives only `Debug`. Dependencies are always collected; the `--Mall` file and its argument go away, and the S0102 check that rejects `--Mall` in user arguments now also rejects the binary-only options listed above. A `slang_sys::Error` becomes a `SlangError` with one `Fatal` diagnostic carrying the exception text, `rendered` set to that text, and empty `deps`. Source paths in the AST are still made absolute. `slang_version()` returns `SLANG_VERSION`, so specialization cache keys still include the version. `Diag` is re-exported from `slang-sys`. Removed: `SLANG_PATH`, `find_slang`, the binary check, the temporary JSON and dependency files, and `capture_stdio`. `str2tmpfile` stays (the parser and tests use it).

### Sandstitch

`src/import/typed.rs` and `src/import/specialize.rs` (`slang_failure`, `typed::slang_errors`) read `SlangError::diagnostics` instead of splitting the error text at `stderr: `. They keep mapping errors to override lines by file and line and matching Slang's message text (`undeclared identifier`), which is unchanged; the raw-text fallback uses `rendered`. `specialize.rs` takes dependencies from `SlangOutput::deps` and `SlangError::deps` instead of reading a `--Mall` file. S0102 keeps showing Slang's text for a missing source file.

### Tooling

`cargo xtask slang`, its download and cache code and the `sha2` use it needs are removed; `cargo xtask ci` just builds, and the library boundary check accepts `slang-sys` as a dependency of `slang-rs` only. Tests that elaborate emitted RTL (`elaborate_manifest` in the FlooNoC tests, the Slang checks in `tests/typed/emit.rs`) use a test helper on `slang_sys::run` instead of the binary. `CLAUDE.md` and `README.md` drop the Slang download notes.

### Testing

All existing import, diagnostic snapshot and FlooNoC tests pass. Snapshots change only where they quoted the old error text; each change is checked. New tests:

- A parse error returns structured diagnostics with absolute file and line.
- A missing source file returns a `Fatal` diagnostic carrying Slang's text, and nothing is written to stderr.
- An error inside a macro expansion reports the original location.
- `deps` includes an `` `include ``d header, has no duplicates and no empty paths.
- A binary-only option in `extra_arguments` is rejected with S0102.
- `rendered` contains no ANSI escapes when stderr is a terminal (run under a pseudo-terminal).
- Two threads importing different modules at once both succeed with the same results as one after the other.
- A deeply nested fixture imports from a thread with a 2 MiB stack.
- A nonexistent `-I` directory gives a warning and the import still succeeds.
- A failed assertion inside `libsvlang` returns `Err`: a test-only shim entry point calls a Slang library function whose precondition `SLANG_ASSERT` checks, with input that violates it.

## Step 2: typed queries (#18)

The shim gains typed queries that walk Slang's symbols instead of serializing them: ports of a top or instance path, parameters, packages, and the instance hierarchy, with types as a tree built from Slang's type system: packed and unpacked arrays with ranges, packed structs and unions with fields, enums with variants (value, width, signedness), and package-qualified names.

`slang-rs` keeps `Type`, `Field`, `Range`, `Variant`, `Port`, `ParameterDef`, `Package` and `Instance` as its public shape and fills them from the shim. The areas move one at a time: ports, types, parameters, packages, hierarchy. While both paths exist, a differential test runs every import fixture and the FlooNoC router through both and requires equal results. When all areas match, `AstJson`, `TypeResolver`, the pest grammar, the literal parsing and the `serde_json` AST handling are removed.

Import time and peak memory of the FlooNoC mesh are recorded before step 1 and after step 2. This step's details are settled when #18 starts and recorded here.

## Out of scope

- General Rust bindings for Slang.
- Linking a prebuilt Slang library (decision 6).
- A command line tool; whether the explorer (#8) needs a small serving binary is open on #9.
- Python bindings; the static, position-independent library is the only preparation.
- Prebuilt Slang artifacts published by this project.
