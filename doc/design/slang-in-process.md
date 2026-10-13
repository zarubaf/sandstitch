# Slang in-process

Status: accepted, 2026-10-13; step 1 implemented (#17), step 2 (#18) designed. Tracks #5.

## Goal

Sandstitch links Slang into the library and calls it in-process. Sandstitch is used as a crate in downstream RTL projects, and later through Python bindings; it is not shipped as a prebuilt command line tool, so Slang has to ship with the crate.

## Decisions

1. **Link, don't spawn.** Slang is compiled from pinned vendored source and linked statically. The crate does not use any `slang` the user has installed; a project's own Slang for linting or simulation is independent, as with any other frontend.
1. **Three layers.** `topstitch` import (model concepts) → `slang-rs` (safe typed Rust API, no model concepts) → `slang-sys` (vendored C++, build, `cxx` shim). Slang details stay out of the model and out of the library boundary; `topstitch` does not depend on `slang-sys` directly.
1. **A narrow shim, not general bindings.** The shim answers the importer's questions and returns plain data. Binding Slang's object graph to Rust is out of scope.
1. **Two steps.** Step 1 (#17) is implemented: Slang runs in-process and the JSON extractors are kept, so every existing test checks the integration. Step 2 (#18) replaces the JSON AST with a typed snapshot of the elaborated design, checked against the JSON path until it is removed.
1. **`slang-rs` diverges from upstream.** xlsynth's `slang-rs` stays subprocess and JSON based. Its fixes are ported by hand; `doc/fork.md` records this.
1. **One way to build.** The crate always builds its vendored Slang. Linking a prebuilt library is left out until someone needs it: a foreign build can differ in allocator, Boost, position independence and assertion settings in ways that cannot be checked reliably at link time.

## Build (`crates/slang-sys`)

**Sources.** Slang v11.0 and fmt 12.1.0, unpacked under `crates/slang-sys/vendor/`, with their license files. Of Slang, `include/`, `source/`, `external/`, `cmake/`, `scripts/` (the diagnostic and syntax generators run at build time) and the CMake files are kept; tests, docs, `pyslang/`, `tools/` and `bindings/` are not included. `doc/fork.md` records the versions and the update procedure.

**CMake.** `build.rs` copies the vendored sources into `OUT_DIR`, only when they differ from the existing copy and keeping their timestamps (a needless copy would reconfigure and rebuild Slang), and builds there with the `cmake` crate, with `no_default_flags(true)` so the Cargo profile does not inject `-g` or optimization flags:

- `CMAKE_BUILD_TYPE=Release` for every Cargo profile.
- `SLANG_INCLUDE_TOOLS`, `SLANG_INCLUDE_TESTS`, `SLANG_INCLUDE_INSTALL` and `SLANG_USE_MIMALLOC` off; `CMAKE_POSITION_INDEPENDENT_CODE=ON` (Python extension modules later).
- `SLANG_ASSERT_ENABLED=1` for the library and the shim alike: a failed internal assertion throws `AssertionException`, which reaches Rust as an error instead of undefined behavior.
- `CMAKE_CXX_SCAN_FOR_MODULES=OFF`: CMake 4 scans C++20 sources for modules by default, which ccache cannot cache.
- Vendored dependencies only: `FETCHCONTENT_SOURCE_DIR_FMT` pointing to the copied fmt, `FETCHCONTENT_FULLY_DISCONNECTED=ON` and `FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER` (no network, no system fmt), and `CMAKE_DISABLE_FIND_PACKAGE_Boost=ON` (always the vendored single-header `boost_unordered`, matching the shim's `SLANG_BOOST_SINGLE_HEADER`).

The shim is compiled with `cxx-build` against the copied headers and the generated ones in the CMake build directory (`slang_export.h`, syntax and diagnostic headers), with `SLANG_STATIC_DEFINE`, `SLANG_BOOST_SINGLE_HEADER`, `SLANG_USE_THREADS` and `SLANG_ASSERT_ENABLED=1`, and links `svlang`, `fmt` and the C++ standard library. A test (`crates/slang-sys/tests/version.rs`) checks that the linked library's `VersionInfo` reports `SLANG_VERSION`; the git hash in Slang's version string is `0`, since the vendored copy has no Slang history, which is expected.

**Compiler cache.** If `ccache` is on `PATH`, `build.rs` sets it as the C and C++ compiler launcher with `CCACHE_BASEDIR` set to `OUT_DIR` and `CCACHE_NOHASHDIR=1`. Sources, generated headers and objects all live under `OUT_DIR`, so every path is relative to it and the cache hits across profiles, targets, `cargo clean` and other projects using the same version. `SLANG_SYS_NO_CCACHE=1` disables this. sccache is not configured: it does not rewrite paths, so it would not hit across `OUT_DIR`s. ccache hits across build directories only when paths are rewritten relative to a common base directory, hence `CCACHE_BASEDIR`.

**Requirements.** C++20 with GCC 11 or Clang 17 or newer, CMake 3.20 or newer, and Python 3 (Slang generates sources at build time). `build.rs` checks all three before configuring and fails with a message naming the requirement and what it found; for CMake it suggests `pip install cmake`, which needs no root. The C++ standard library is linked dynamically, so a binary built with GCC 11 needs at least that libstdc++ at run time. Linux x86-64 is tested; other targets are not blocked, but are documented as untested.

**CI.** `.github/workflows/ci.yml` installs ccache and caches its directory and Cargo's target directory, so ordinary runs do not rebuild Slang. There is no separate native build job.

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

**Diagnostics.** `Driver` prints some problems directly instead of reporting them as diagnostics: unknown options, missing source files, a bad `--timescale` and unreadable `-f` files as errors; a missing include directory or `--suppress-warnings` path as warnings; and parse progress to stdout. These are captured: constructing the `Driver`, turning colors off, option processing and stage 1 run under one process-wide mutex with `OS::captureOutput` on, and both captured streams are drained before the mutex is released. The captured stderr is split at the `error: `, `warning: ` and `note: ` prefixes `Driver` writes; errors become `Fatal` diagnostics without a file, warnings and notes keep their severity, so an import that the binary accepts with a warning still succeeds. Diagnostics reach the shim through Slang's diagnostic engine, so `-W…`, `-Werror`, `--suppress-warnings`, pragmas and the error limit apply: the shim replaces the driver's text client with a recording client and an uncolored text client of its own, issues the syntax trees' diagnostics itself for `ParseOnly`, and the compilation's (which include the parse diagnostics) for `Elaborate`. Command line errors become `Fatal`. A source that fails to load is `Fatal` but, as in `slang`, the run elaborates the rest; only failed option processing skips elaboration. Elaboration, analysis and serialization run outside the mutex, in parallel; this is sound only because nothing after stage 1 prints, so the shim never calls Slang's reporting functions (`reportParseDiags`, `reportCompilation`, `reportDiagnostics`, `runFullCompilation`, `optionallyWriteDepFiles`), whose output would land in another call's capture. The mutex serializes parsing across concurrent imports. Locations are mapped to where the text is written (inside a macro, its definition; `rendered` keeps Slang's use-site context), and file names are made absolute lexically by the Rust side of `slang-sys` (`absolute_path`, without resolving symbolic links), the same function `slang-rs` applies to AST source paths, so all three agree. Options whose effect lives only in those functions (`--Mall`, `--Minclude`, `--Mmodule`, `--depfile-target`, `--diag-json`, in either spelling) would silently do nothing, so the shim reports each as `Fatal`, in `diagnostics` and in `rendered`, after option processing and skips elaboration. All text fields cross the boundary with `rust::String::lossy`, so invalid UTF-8 in a diagnostic becomes U+FFFD instead of failing the run. `rendered` uses Slang's text client with colors forced off; the shim never writes to the process's stdout or stderr.

**Isolation.** Each call runs on its own thread with a 64 MiB stack, joined before `run` returns, so deep nesting does not depend on the caller's stack size (2 MiB for test and spawned threads). Each call builds its own `Driver` and `Compilation`; after constructing the `Driver` the shim turns terminal colors off, since the constructor sets them from whether stderr is a terminal. A C++ exception, including a failed internal assertion, becomes `Err`, except where Slang asserts inside a `noexcept` function or destructor, which terminates. A crash in Slang that is not an exception still ends the host process; this is the cost of running in-process.

### `slang-rs`

The run functions return typed results:

```rust
pub struct SlangOutput { pub ast: Ast, pub ok: bool, pub deps: Vec<PathBuf> }
pub struct SlangError  { pub diagnostics: Vec<Diag>, pub rendered: String, pub deps: Vec<PathBuf> }

pub fn run_slang(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>;           // ok is true
pub fn run_slang_keep_ast(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>; // ok may be false
pub fn check_slang(cfg: &SlangConfig) -> Result<(), SlangError>;                 // Stage::ParseOnly
```

`Ast` is a newtype over `serde_json::Value` that dereferences to it and drops iteratively, so deep ASTs do not overflow small stacks; it derives only `Debug`. Dependencies are always collected from the shim's `deps`, without a `--Mall` file. The S0102 check rejects `--Mall` and the binary-only options listed above in user arguments. A `slang_sys::Error` becomes a `SlangError` with one `Fatal` diagnostic carrying the exception text, `rendered` set to that text, and empty `deps`. Source paths in the AST are made absolute. `slang_version()` returns `SLANG_VERSION`, so specialization cache keys include the version. `Diag` is re-exported from `slang-sys`. No `slang` executable or `SLANG_PATH` is used, and no temporary JSON or dependency files are written. Type structure is still read from the JSON AST and parsed with a regex resolver and a pest grammar; #18 replaces this. `str2tmpfile` is used by the parser and tests.

### Sandstitch

`src/import/typed.rs` and `src/import/specialize.rs` (`slang_failure`, `typed::slang_errors`) read `SlangError::diagnostics`. They map errors to override lines by file and line and match Slang's message text (`undeclared identifier`); the raw-text fallback uses `rendered`. `specialize.rs` takes dependencies from `SlangOutput::deps` and `SlangError::deps`. S0102 keeps showing Slang's text for a missing source file.

### Tooling

`cargo xtask ci` builds Slang as part of the workspace, and the library boundary check accepts `slang-sys` as a dependency of `slang-rs` only. Tests that elaborate emitted RTL (`elaborate_manifest` in the FlooNoC tests, the Slang checks in `tests/typed/emit.rs`) use a test helper on `slang_sys::run`.

### Testing

The import, diagnostic snapshot and FlooNoC tests cover the in-process path. Further tests cover:

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

The shim walks Slang's elaborated symbols and returns a typed snapshot of the design. `slang-sys` defines the snapshot's Rust types and its queries once; `slang-rs` re-exports them, as it does `Diag`.

### Snapshot

A `run` with `Stage::Elaborate` and `design` requested builds the snapshot before it returns, on the same thread; nothing of Slang outlives the call, so isolation is as in step 1. A live handle to the `Compilation` would tie Rust lifetimes to Slang's object graph and call into Slang outside that thread; the snapshot holds only what the importer reads.

```rust
pub struct Request {
    pub args: Vec<String>,
    pub stage: Stage,
    pub design: DesignQuery,     // None | Structure | WithSignals
}

pub struct Response {
    pub ok: bool,
    pub design: Option<Design>,  // Some when requested and a compilation was created
    pub deps: Vec<String>,
    pub diagnostics: Vec<Diag>,
    pub rendered: String,
}
```

`DesignQuery::Structure` collects instances, ports, parameters and packages; `WithSignals` adds the nets and variables of each body, which only the tests that check emitted RTL read. A run whose options fail or that creates no compilation returns `design: None` whatever was requested. While the JSON path exists (see Migration), `Request` also carries `ast_json` and `Response` the serialized AST.

### What the shim walks

The walker (`shim/design.cc`) follows Slang's AST serializer in what it visits, so that the snapshot covers what the JSON AST covered:

- **Tops.** `root.topInstances`, in order.
- **Bodies.** Each `InstanceBodySymbol` once; instances that share a body refer to it. An instance whose body is flagged `InstanceFlags::Uninstantiated` is left out entirely, body and child entry alike. A body's direct members give:
  - ports in declaration order: `PortSymbol` (name, direction, type, location) and `InterfacePortSymbol` (name, interface definition name, modport, location). A `ref` port or a `MultiPortSymbol` is recorded as unsupported, never dropped;
  - parameters: `ParameterSymbol` (name, type, local flag) and `TypeParameterSymbol` (name, local flag);
  - children: `InstanceSymbol` (instance name, definition name, body), `UninstantiatedDefSymbol` (instance name, definition name), and the members of instantiated `GenerateBlockSymbol`s and valid `GenerateBlockArraySymbol`s, flattened into the body with a hierarchical prefix. A loop generate element's prefix is `name[index]` with the genvar value (`arrayIndex`). Instance arrays are not part of the hierarchy;
  - with `WithSignals`, `NetSymbol` and `VariableSymbol` (name, Slang's display of the type, `getBitstreamWidth()`).
- **Packages.** The `PackageSymbol` members of `root.compilationUnits`: value parameters (name, value as `toString(SVInt::MAX_BITS, true, true)`, the serializer's form) and `TypeAliasType` members (name, target type, and the alias printed by Slang's type printer with `skipTypeDefs` off, such as `struct packed{logic[1:0] a;}my_pkg::my_struct_t`).

**Locations** are those of the JSON source info: the file name and line of the symbol's fully expanded location (`getFileName`, `getLineNumber`, so `` `line `` directives apply), the column of its raw location (`getColumnNumber`, 0 inside a macro, which becomes `None`). File names are made absolute with `absolute_path`, except pseudo-files such as `<command-line>`.

### Types

Types are walked as written, not canonicalized, so that names survive:

| Slang type | `Type` |
|---|---|
| `ScalarType` (`logic`, `reg`, `bit`), `PredefinedIntegerType` (`int`, `byte`, ...) | `Logic` with signedness and its packed range |
| `PackedArrayType`, `FixedSizeUnpackedArrayType` | the element type with the range added to `packed_dimensions` or `unpacked_dimensions`, outermost first |
| `PackedStructType`, `PackedUnionType` | `Struct` / `Union`, `packed: true`, fields in declaration order |
| `UnpackedStructType`, `UnpackedUnionType` | `Struct` / `Union`, `packed: false` |
| `EnumType` | `Enum`: variants with name, value as a `BigInt`, and the width and signedness of the base type |
| `TypeAliasType` | its target, see names |

**Names.** A struct, union or enum is named by the outermost alias that reaches it through aliases only, as Slang's type printer writes that alias (`cfg_pkg::flit_t`; `top.byte_t` for a typedef in a module). An alias whose target is an array names the array, not the element, so the element keeps its own name (`typedef flit_t [1:0] pair_t` gives elements named `flit_t`). The alias a type parameter introduces is transparent: a port of type `T` with `T = cfg_pkg::flit_t` is named `cfg_pkg::flit_t`. A struct, union or enum without an alias is named as Slang names anonymous types: the lexical path of its scope followed by `s$`, `u$` or `e$` and Slang's numeric id (`m.s$3`).

Any other type (`real`, `string`, dynamic and associative arrays, queues, `chandle`, class and virtual interface types, an error type) makes the port unsupported, with Slang's display of the type as `type_text` and the reason as `error`; so does an enum value with X or Z bits.

### Bridge

cxx cannot express recursive shared structs, so the design crosses as flat tables in `FfiResponse`: types (kind, name, signedness, ranges, and indices of element, field and base types, each smaller than the type's own), bodies (ports, parameters, signals and children, children referring to bodies), tops (instance name and body) and packages. Field names and alias names are strings; an enum value is its two's-complement bytes, little-endian, with the width and signedness alongside. `slang-sys` converts the tables into the public types and checks that every index is in range and smaller than the referring entry's, so that a malformed table is an `Error` and cannot cycle. Bodies are therefore numbered after their children (post-order). Since a struct, union or enum's name depends on the alias path that reached it, types are shared in the table only when both the Slang type and the name are the same.

### Queries

```rust
impl Design {
    pub fn tops(&self) -> impl Iterator<Item = &str>;
    pub fn ports(&self, path: &str) -> Option<Vec<Result<Port, UnsupportedPort>>>;
    pub fn interface_ports(&self, path: &str) -> Option<Vec<InterfacePort>>;
    pub fn parameters(&self, path: &str) -> Option<Vec<ParameterDef>>;
    pub fn overridable_parameters(&self, path: &str) -> Option<Vec<(String, ParameterKind)>>;
    pub fn signals(&self, path: &str) -> Option<Vec<Signal>>;
    pub fn hierarchy(&self) -> HashMap<String, Instance>;
    pub fn packages(&self) -> HashMap<String, Package>;
}
```

`path` is a top-level module name or a dot-separated path of instance names through child instances that are not inside generate blocks. `ports` excludes interface ports, which `interface_ports` returns. `parameters` includes localparams and leaves out parameters whose type is unsupported, which callers report as having no integer literal form (S0105); `overridable_parameters` lists the non-local value and type parameters, whatever their types. `Type`, `Field`, `Range`, `Variant`, `Port`, `PortDir`, `InterfacePort`, `UnsupportedPort`, `ParameterDef`, `ParameterKind`, `SourceLoc` and `Instance` keep their current shape. A package `Parameter` has `name`, `value` (the constant, or the alias as printed above; `logic[33:22]` for a plain vector) and `ty: Option<Type>`, set for aliases. Sandstitch's own `Parameter` (`src/import/package.rs`) keeps `slang-rs` types out of its API (decision 2): it stores the width of an alias's type in a private field at import, `calc_type_width` returns it, and `Parameter::new(name, value)` builds a parameter without one.

### `slang-rs`

`SlangOutput` holds `design: Design` in place of the AST. `run_slang` returns it when the design elaborated without errors. `run_slang_keep_design` returns it whenever a compilation was created, with `ok` false after errors; every declared member is kept, and members whose types Slang could not resolve are unsupported. It returns the failure when no compilation was created (failed options, a missing source), so S0102 still reports Slang's text. `check_slang` is unchanged. `extract_packages` runs Slang and returns `packages()`.

`slang-rs` has no JSON AST, `TypeResolver`, type grammar or literal parsing, and depends on neither `serde`, `serde_json`, `serde_stacker`, `pest`, `pest_derive` nor `regex`. The shim does not call `ASTSerializer`.

### Fixed behavior

The JSON path's results differ from the snapshot's in these cases, each a defect of the JSON path, each covered by its own test:

1. `UnsupportedPort::type_text` and `error` are Slang's display of the type and the reason.
1. A loop generate block's prefix uses the genvar value: `for (genvar i = 2; ...) begin : g` gives `g[2]`, not `g[0]`.
1. A typedef of a type parameter resolves to the parameter's type instead of being unsupported.
1. A `ref` port is unsupported instead of panicking; a `MultiPortSymbol` port is unsupported instead of dropped.
1. A package alias of a struct, union or enum prints with its package-qualified name (`...}my_pkg::my_struct_t`), as port types are named, instead of the bare alias name.

### Migration

Areas move in this order, one commit each: instance paths with ports and types, parameters, packages, hierarchy, signals, then removal of the JSON path. Each commit switches Sandstitch's callers for its area to the snapshot, so the existing tests exercise the typed values from the first commit on.

While both paths exist, the `differential` feature of `slang-rs`, enabled by the dev-dependencies of `slang-rs`, Sandstitch and `designs/floonoc`, requests both the JSON AST and the snapshot from every run and panics if any switched area differs. The comparison builds the JSON extractors' answers once per run for all tops and instances and applies the fixed-behavior list above as an explicit normalization. It also requires both paths to agree on whether a run fails. Library builds without the feature run only the snapshot.

### Testing

- **Generated designs** (`crates/slang-rs/tests/generated.rs`, `proptest`). A strategy generates a model of a package and a module hierarchy: ports and parameters of nested packed and unpacked structs and unions, enums (implicit, explicit, negative and wide values; signed and unsigned base types), packed and unpacked arrays with ascending, descending and negative ranges, typedefs in packages and modules, typedefs of arrays of named types, widths from parameters, type parameters with typedefs of them, child instances and loop generate blocks with non-zero start values. A printer writes it as SystemVerilog. Every generated design must elaborate without diagnostics; a diagnostic fails the case as a generator defect. The test compares every query with the model, which encodes SystemVerilog's meaning, not either implementation; names of anonymous types are compared up to Slang's numeric id; while the JSON path exists the cases also run the differential comparison. CI runs proptest's default case count; `PROPTEST_CASES` raises it; failures shrink and are kept in `proptest-regressions/`.
- **Corpus** (`crates/slang-rs/tests/corpus.rs`). Three runs, one per library: `third_party/common_cells/src`, `third_party/axi/src` and `third_party/floonoc/hw`, without `deprecated/`, `test/`, `tb/`, `formal/` and `synth/`, each with its dependencies and include directories as in `designs/floonoc/src/lib.rs`. Every module of the library is passed as a `--top`, with `--allow-toplevel-iface-ports`, through `run_slang_keep_design`, and every query is answered for every top and instance. Tops defined in a file with an error diagnostic are left out of the comparison, and the test asserts that at least a recorded number of tops remain, so that a regression in options or sources cannot reduce it to comparing error recovery. The test stays under 30 seconds in a debug build with the differential comparison on. While the JSON path exists this is the differential test on real code.
- The existing `slang-rs`, import, diagnostic and FlooNoC tests, ported from the removed functions to the queries, with expectations changed only by the fixed-behavior list.
- A `slang-sys` unit test feeds tables with out-of-range and forward indices to the conversion and expects `Error`.

### Measurement

Import time and peak memory of the big FlooNoC 2x2 mesh (`cargo test --release -p floonoc --test bench -- --ignored --nocapture`, median of 3) are recorded on #18, against 457 ms elapsed and 97856 KiB own peak after step 1.

## Out of scope

- General Rust bindings for Slang.
- Linking a prebuilt Slang library (decision 6).
- A command line tool; whether the explorer (#8) needs a small serving binary is open on #9.
- Python bindings; the static, position-independent library is the only preparation.
- Prebuilt Slang artifacts published by this project.
