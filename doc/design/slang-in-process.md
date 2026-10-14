# Slang in-process

Status: accepted, 2026-10-13; step 1 implemented (#17), step 2 implemented (#18). Tracks #5.

## Goal

Sandstitch links Slang into the library and calls it in-process. Sandstitch is used as a crate in downstream RTL projects, and later through Python bindings; it is not shipped as a prebuilt command line tool, so Slang has to ship with the crate.

## Decisions

1. **Link, don't spawn.** Slang is compiled from pinned vendored source and linked statically. The crate does not use any `slang` the user has installed; a project's own Slang for linting or simulation is independent, as with any other frontend.
1. **Three layers.** `topstitch` import (model concepts) → `slang-rs` (safe typed Rust API, no model concepts) → `slang-sys` (vendored C++, build, `cxx` shim). Slang details stay out of the model and out of the library boundary; `topstitch` does not depend on `slang-sys` directly.
1. **A narrow shim, not general bindings.** The shim answers the importer's questions and returns plain data. Binding Slang's object graph to Rust is out of scope.
1. **Two steps.** Step 1 (#17): Slang runs in-process. Step 2 (#18): the importer reads a typed snapshot of the elaborated design that the shim builds.
1. **`slang-rs` diverges from upstream.** xlsynth's `slang-rs` stays subprocess and JSON based. Its fixes are ported by hand; `doc/fork.md` records this.
1. **One source, optionally prebuilt.** The crate builds its vendored Slang, or links a library that this crate's own build made (`SLANG_SYS_PREBUILT_DIR`, see "Prebuilt"), so that consumers' CI does not rebuild Slang with CMake. A foreign Slang build (a distribution's, another project's) is not accepted: it can differ in allocator, Boost, position independence and assertion settings in ways that cannot be checked reliably at link time.

## Build (`crates/slang-sys`)

**Sources.** Slang v12.0 and the three dependencies its CMake fetches (fmt 12.2.0, toml++ 3.4.0 and the header-only `boost_regex` fork at tag `boost-1.91.0`), unpacked under `crates/slang-sys/vendor/` with their license files. Of Slang, `include/`, `source/`, `external/`, `cmake/`, `scripts/` (the diagnostic and syntax generators run at build time) and the CMake files are kept; tests, docs, `examples/`, `pyslang/`, `tools/` and `bindings/` are not included. `doc/fork.md` records the versions and the update procedure.

**CMake.** `build.rs` copies the vendored sources into `OUT_DIR`, only when they differ from the existing copy and keeping their timestamps (a needless copy would reconfigure and rebuild Slang), and builds there with the `cmake` crate, with `no_default_flags(true)` so the Cargo profile does not inject `-g` or optimization flags:

- `CMAKE_BUILD_TYPE=Release` for every Cargo profile.
- `SLANG_INCLUDE_TOOLS`, `SLANG_INCLUDE_TESTS`, `SLANG_INCLUDE_INSTALL` and `SLANG_USE_MIMALLOC` off, and `SLANG_USE_SYSTEM_FMT`, `SLANG_USE_SYSTEM_BOOST` and `SLANG_USE_SYSTEM_TOMLPLUSPLUS` set explicitly off (we always use the vendored copies); `CMAKE_POSITION_INDEPENDENT_CODE=ON` (Python extension modules later).
- `SLANG_ASSERT_ENABLED=1` for the library and the shim alike: a failed internal assertion throws `AssertionException`, which reaches Rust as an error instead of undefined behavior.
- `CMAKE_CXX_SCAN_FOR_MODULES=OFF`: CMake 4 scans C++20 sources for modules by default, which ccache cannot cache.
- Vendored dependencies only: `FETCHCONTENT_SOURCE_DIR_FMT`, `FETCHCONTENT_SOURCE_DIR_TOMLPLUSPLUS` and `FETCHCONTENT_SOURCE_DIR_BOOST_REGEX` pointing to the copied sources, `FETCHCONTENT_FULLY_DISCONNECTED=ON` and `FETCHCONTENT_TRY_FIND_PACKAGE_MODE=NEVER` (no network, no system packages). `SLANG_USE_SYSTEM_BOOST=OFF` selects the vendored single-header `boost_unordered`, matching the shim's `SLANG_BOOST_SINGLE_HEADER`.

The shim is compiled with `cxx-build` against the copied headers and the generated ones in the CMake build directory (`slang_export.h`, syntax and diagnostic headers), with `SLANG_STATIC_DEFINE`, `SLANG_BOOST_SINGLE_HEADER`, `SLANG_USE_THREADS` and `SLANG_ASSERT_ENABLED=1`, and links `svlang` (Slang compiles the fmt implementation into it from the vendored fmt headers) and the C++ standard library. A test (`crates/slang-sys/tests/version.rs`) checks that the linked library's `VersionInfo` reports `SLANG_VERSION`; the git hash in Slang's version string is `0`, since the vendored copy has no Slang history, which is expected.

**Compiler cache.** If `ccache` is on `PATH`, `build.rs` sets it as the C and C++ compiler launcher with `CCACHE_BASEDIR` set to `OUT_DIR` and `CCACHE_NOHASHDIR=1`. Sources, generated headers and objects all live under `OUT_DIR`, so every path is relative to it and the cache hits across profiles, targets, `cargo clean` and other projects using the same version. `SLANG_SYS_NO_CCACHE=1` disables this. sccache is not configured: it does not rewrite paths, so it would not hit across `OUT_DIR`s. ccache hits across build directories only when paths are rewritten relative to a common base directory, hence `CCACHE_BASEDIR`.

**Requirements.** C++20 with GCC 11 or Clang 17 or newer, CMake 3.28 or newer, and Python 3 (Slang generates sources at build time). `build.rs` checks all three before configuring and fails with a message naming the requirement and what it found; for CMake it suggests `pip install cmake`, which needs no root. The C++ standard library is linked dynamically, so a binary built with GCC 11 needs at least that libstdc++ at run time. Linux x86-64 is tested; other targets are not blocked, but are documented as untested.

**Prebuilt.** `cargo xtask slang-prebuilt <DIR>` builds `slang-sys` from source and copies `libsvlang.a` (`lib/`), the headers the shim compiles against (`include/`: Slang's `include/` and `external/` and the generated headers), the license files and the manifest `slang-prebuilt.toml` into `<DIR>/slang-<release>-<target>-<compiler><major>`, and prints that path. Every source build writes the manifest to `OUT_DIR`; the command only collects it. With `SLANG_SYS_PREBUILT_DIR` set to such a directory (an absolute path), `build.rs` skips the vendored copy, CMake and the CMake and Python checks, compiles the shim against `include/` with the same defines and links `lib/`. It first checks the manifest against the current build and stops with one line per mismatch:

| Key | Value | The current build must have |
|---|---|---|
| `slang` | vendored Slang release | the same |
| `target` | Rust target triple | the same |
| `compiler`, `compiler_version` | `gcc`, `clang`, `apple-clang`; version | the same family; at least the major version |
| `stdlib`, `stdlib_version` | `libstdc++` (`_GLIBCXX_RELEASE`) or `libc++` (`_LIBCPP_VERSION`) | the same library; at least that version |
| `cxx11_abi` | `_GLIBCXX_USE_CXX11_ABI`, `none` outside libstdc++ | the same |
| `glibc` | `__GLIBC__.__GLIBC_MINOR__`, `none` for another C library | at least that version; `none` only with `none` |
| `std`, `pic`, `defines` | `c++20`, position-independent code, the shim's defines (`SLANG_ASSERT_ENABLED=1` and the others above) | the same |

The standard library and glibc values come from preprocessing a probe source with the same compiler and C++ standard as the shim (`-P`; the parser also reads GCC's output with line markers). "At least" holds because libstdc++, libc++ and glibc keep old symbols when they add new ones: a library compiled against older headers links with a newer runtime, not the reverse. The compiler's major version is compared for the same reason: a compiler's support libraries (libgcc, compiler-rt) follow that rule too. For libstdc++ only the major version (`_GLIBCXX_RELEASE`) is compared; a prebuilt from a later minor release can need a symbol version that an earlier one lacks, which fails at link time with an undefined symbol, not silently. Where the compiler is not recognized, a source build writes no manifest (with a warning) and a prebuilt cannot be used.

A static library does not remove the match of the C++ and C libraries. `libsvlang.a` is object code compiled against one standard library's headers: its `std::string`, `std::vector` and exception types have that library's layout and mangled names, and it calls that library's out-of-line functions and the C library's functions (with glibc, versioned symbols such as `__isoc23_strtol@GLIBC_2.38`). Neither library is inside the archive; the final binary links the consumer's at link and run time, so they must be compatible with the headers Slang saw: the same standard library with the same dual ABI setting (libstdc++'s `std::string` before and after the C++11 ABI are different types), at least as new, and a C library of the same kind. glibc and musl are different C libraries with different ABIs, so a musl target (`x86_64-unknown-linux-musl`) needs its own prebuilt, built on musl; the target triple check catches the mismatch. To serve older systems, build the prebuilt on the oldest glibc and compiler to support.

**CI.** `.github/workflows/ci.yml` installs ccache and caches its directory and Cargo's target directory, so ordinary runs do not rebuild Slang. `.github/workflows/slang-prebuilt.yml` runs `cargo xtask slang-prebuilt` on a `v*` tag or a manual dispatch on Ubuntu 24.04 (GCC 13, glibc 2.39) and uploads the directory as a `.tar.gz` artifact named like the directory; on a tag it also attaches it to that release.

## Step 1: Slang in-process (#17)

### Shim

One C++ file and a `cxx` bridge in `slang-sys`:

```rust
pub struct Request {
    pub args: Vec<String>,   // Driver options: sources, -I, -D, -G, --top, -y, ... (no argv[0])
    pub stage: Stage,        // ParseOnly | Elaborate
    pub design: DesignQuery, // None | Structure | WithSignals; see step 2
}

pub struct Response {
    pub ok: bool,                // no error diagnostics in the stages run
    pub design: Option<Design>,  // Some when requested and a compilation was created, also when !ok
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
pub const SLANG_VERSION: &str = "12.0";                // the vendored release
```

**Options.** `args` holds only options registered by `driver::Driver`, parsed by `Driver::parseCommandLine` as the `slang` binary parses them. Options that only the binary's `main` defines (`--ast-json`, `--ast-json-source-info`, `--parse-only`, `--disable-analysis`, `-q`) are not accepted in `args`; `stage` and `design` take their place. Unless `args` sets the thread count in any form (`-j4`, `-j 4`, `--threads 4`, `--threads=4`), the shim adds `--threads 1`, so concurrent calls do not each start a thread pool.

**Stages,** in the order `slang` runs them:

1. Process options and load and parse all sources. Option, file and parse errors are recorded. With `Stage::ParseOnly`, stop here.
1. Create the compilation and elaborate.
1. Run analysis, as the binary does without `--disable-analysis`.
1. If requested, walk the elaborated design into the snapshot (step 2), including after errors, so that `run_slang_keep_design` keeps working.

**Diagnostics.** `Driver` prints some problems directly instead of reporting them as diagnostics: unknown options, missing source files, a bad `--timescale` and unreadable `-f` files as errors; a missing include directory or `--suppress-warnings` path as warnings; and parse progress to stdout. These are captured: constructing the `Driver`, turning colors off, option processing and stage 1 run under one process-wide mutex with `OS::captureOutput` on, and both captured streams are drained before the mutex is released. The captured stderr is split at the `error: `, `warning: ` and `note: ` prefixes `Driver` writes; errors become `Fatal` diagnostics without a file, warnings and notes keep their severity, so an import that the binary accepts with a warning still succeeds. Diagnostics reach the shim through Slang's diagnostic engine, so `-W…`, `-Werror`, `--suppress-warnings`, pragmas and the error limit apply: the shim replaces the driver's text client with a recording client and an uncolored text client of its own, issues the syntax trees' diagnostics itself for `ParseOnly`, and the compilation's (which include the parse diagnostics) for `Elaborate`. Command line errors become `Fatal`. A source that fails to load is `Fatal` but, as in `slang`, the run elaborates the rest; only failed option processing skips elaboration. Elaboration, analysis and the design walk run outside the mutex, in parallel; this is sound only because nothing after stage 1 prints, so the shim never calls Slang's reporting functions (`reportParseDiags`, `reportCompilation`, `reportDiagnostics`, `runFullCompilation`, `optionallyWriteDepFiles`), whose output would land in another call's capture. The mutex serializes parsing across concurrent imports. Locations are mapped to where the text is written (inside a macro, its definition; `rendered` keeps Slang's use-site context), and file names are made absolute lexically by the Rust side of `slang-sys` (`absolute_path`, without resolving symbolic links), the same function that makes the snapshot's source paths absolute, so all agree. Options whose effect lives only in those functions (`--Mall`, `--Minclude`, `--Mmodule`, `--depfile-target`, `--diag-json`, in either spelling) would silently do nothing, so the shim reports each as `Fatal`, in `diagnostics` and in `rendered`, after option processing and skips elaboration. All text fields cross the boundary with `rust::String::lossy`, so invalid UTF-8 in a diagnostic becomes U+FFFD instead of failing the run. `rendered` uses Slang's text client with colors forced off; the shim never writes to the process's stdout or stderr.

**Isolation.** Each call runs on its own thread with a 64 MiB stack, joined before `run` returns, so deep nesting does not depend on the caller's stack size (2 MiB for test and spawned threads). Each call builds its own `Driver` and `Compilation`; after constructing the `Driver` the shim turns terminal colors off, since the constructor sets them from whether stderr is a terminal. A C++ exception, including a failed internal assertion, becomes `Err`, except where Slang asserts inside a `noexcept` function or destructor, which terminates. A crash in Slang that is not an exception still ends the host process; this is the cost of running in-process.

### `slang-rs`

The run functions return typed results:

```rust
pub struct SlangOutput { pub design: Design, pub ok: bool, pub deps: Vec<PathBuf>, pub diagnostics: Vec<Diag> }
pub struct SlangError  { pub diagnostics: Vec<Diag>, pub rendered: String, pub deps: Vec<PathBuf> }

pub fn run_slang(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>;           // ok is true
pub fn run_slang_keep_design(cfg: &SlangConfig) -> Result<SlangOutput, SlangError>; // ok may be false
pub fn check_slang(cfg: &SlangConfig) -> Result<(), SlangError>;                 // Stage::ParseOnly
```

Dependencies are collected from the shim's `deps`. The S0102 check rejects `--Mall` and the binary-only options listed above in user arguments. A `slang_sys::Error` becomes a `SlangError` with one `Fatal` diagnostic carrying the exception text, `rendered` set to that text, and empty `deps`. `slang_version()` is the constant `slang <SLANG_VERSION>`, so specialization cache keys include the version. `Diag` is re-exported from `slang-sys`. Slang runs in-process; no `slang` executable or `SLANG_PATH` is involved. Types, ports, parameters, the hierarchy and packages come from the snapshot (step 2). `str2tmpfile` is used by the parser and tests.

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

`DesignQuery::Structure` collects instances, ports, parameters and packages; `WithSignals` adds the nets and variables of each body, which the tests that check emitted RTL and the corpus read. A run whose options fail or that creates no compilation returns `design: None` whatever was requested, and so does one that requests `DesignQuery::None`.

### What the shim walks

The walker (`shim/design.cc`) visits:

- **Tops.** `root.topInstances`, in order.
- **Bodies.** Each `InstanceBodySymbol` once; instances that share a body refer to it. An instance whose body is flagged `InstanceFlags::Uninstantiated` is left out entirely, body and child entry alike. A body's direct members give:
  - ports in declaration order: `PortSymbol` (name, direction, type, location) and `InterfacePortSymbol` (name, interface definition name, modport, location). A `ref` port or a `MultiPortSymbol` is recorded as unsupported, never dropped;
  - parameters: `ParameterSymbol` (name, type, local flag) and `TypeParameterSymbol` (name, local flag);
  - children: `InstanceSymbol` (instance name, definition name, body), `UninstantiatedDefSymbol` (instance name, definition name), and the members of instantiated `GenerateBlockSymbol`s and valid `GenerateBlockArraySymbol`s, flattened into the body with a hierarchical prefix. A loop generate element's prefix is `name[index]` with the genvar value (`arrayIndex`). Each element of an `InstanceArraySymbol` is a child named by the array and its indices as declared, `u[3]`, or `u[1][0]` for a multi-dimensional array;
  - with `WithSignals`, `NetSymbol` and `VariableSymbol` among the body's direct members (name, Slang's display of the type, `getBitstreamWidth()`); ports appear as their internal symbols, and members of generate blocks are not included.
- **Packages.** The `PackageSymbol` members of `root.compilationUnits`: value parameters (name, value as `toString(SVInt::MAX_BITS, true, true)`) and `TypeAliasType` members (name, target type, and the alias printed by Slang's type printer with `skipTypeDefs` off, such as `struct packed{logic[1:0] a;}my_pkg::my_struct_t`). Slang's printer cannot print every type: it asserts that every member of a struct or union is a variable, which fails for one that declares an anonymous enum (the enum's values are members of the struct's scope), and it throws on an enum with a value that failed to evaluate. The shim checks a type before printing it (`printable`); the value of an alias it cannot print is its package-qualified name.

**Locations** are the file name and line of the symbol's fully expanded location (`getFileName`, `getLineNumber`, so `` `line `` directives apply), the column of its raw location (`getColumnNumber`, 0 inside a macro, which becomes `None`). File names are made absolute with `absolute_path`, except pseudo-files such as `<command-line>`.

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

**Names.** A struct, union or enum is named by the outermost alias that reaches it through aliases only, as Slang's type printer writes that alias (`cfg_pkg::flit_t`; `top.byte_t` for a typedef in a module). An alias whose target is an array names the array, not the element, so the element keeps its own name (`typedef flit_t [1:0] pair_t` gives elements named `flit_t`). An enum declared in a typedef is named by that typedef, as Slang names it, also when the typedef adds dimensions (`typedef enum {A} e_t [2]` gives elements named `e_t`). The alias a type parameter introduces is transparent: a port of type `T` with `T = cfg_pkg::flit_t` is named `cfg_pkg::flit_t`. A struct, union or enum without an alias is named as Slang names anonymous types: the lexical path of its scope followed by `s$`, `u$` or `e$` and Slang's numeric id (`m.s$3`).

Any other type (`real`, `string`, dynamic and associative arrays, queues, `chandle`, class and virtual interface types, an error type) makes the port unsupported, with Slang's display of the type as `type_text` and the reason as `error`; so does an enum value with X or Z bits (`enum values with X or Z bits are not supported`) and one that is not a constant integer, for example after an undeclared identifier (`enum values that are not constant integers are not supported`); the diagnostics of the run stay available in either case. Slang (MikePopoloski/slang#2017) resolves an anonymous enum port to the error type, without a diagnostic, when an anonymous struct or union port follows it in an ANSI port list (`module top (input enum {A} e, input struct packed {logic f;} s);`); that port is unsupported with `type_text` `<error>`. An anonymous enum declared in a struct or union, in a port or in a package typedef, is walked as any other member type. The same limit applies wherever the walker shows a type as text (`type_text` of an unsupported port, a signal's type): an alias the printer cannot print is shown by its qualified name, an anonymous struct, union or enum as `struct{...}`, `union{...}` or `enum{...}`.

### Bridge

cxx cannot express recursive shared structs, so the design crosses as flat tables in `FfiResponse`: types (kind, name, signedness, ranges, and indices of element, field and base types, each smaller than the type's own), bodies (ports, parameters, signals and children, children referring to bodies), tops (instance name and body) and packages. Field names and alias names are strings; an enum value is its two's-complement bytes, little-endian, with the width and signedness alongside. `slang-sys` converts the tables into the public types and checks that every index is in range and smaller than the referring entry's, so that a malformed table is an `Error` and cannot cycle. Bodies are therefore numbered after their children (post-order). Since a struct, union or enum's name depends on the alias path that reached it, types are shared in the table only when both the Slang type and the name are the same.

### Queries

```rust
impl Design {
    pub fn tops(&self) -> impl Iterator<Item = &str>;
    pub fn ports(&self, path: &str) -> Option<&[Result<Port, UnsupportedPort>]>;
    pub fn interface_ports(&self, path: &str) -> Option<&[InterfacePort]>;
    pub fn child_instances(&self, path: &str) -> Option<Vec<&str>>;
    pub fn parameters(&self, path: &str) -> Option<&[ParameterDef]>;
    pub fn overridable_parameters(&self, path: &str) -> Option<Vec<(String, ParameterKind)>>;
    pub fn signals(&self, path: &str) -> Option<&[Signal]>;
    pub fn hierarchy(&self) -> HashMap<String, Instance>;
    pub fn packages(&self) -> HashMap<String, Package>;
}
```

`child_instances` lists the instantiated children outside generate blocks (array elements as `u[3]`), which the corpus test uses to walk the hierarchy. `path` is a top-level module name or a dot-separated path of instance names through child instances that are not inside generate blocks; an instance array element is named as in the hierarchy (`top.u[3]`). A segment cannot contain a `.`: a child with an escaped name that does (`\u.x `) is listed by `child_instances` but has no path. `ports` excludes interface ports, which `interface_ports` returns. `parameters` includes localparams and leaves out parameters whose type is unsupported, which callers report as having no integer literal form (S0105); `overridable_parameters` lists the non-local value and type parameters, whatever their types. `Type`, `Field`, `Range`, `Variant`, `Port`, `PortDir`, `InterfacePort`, `UnsupportedPort`, `ParameterDef`, `ParameterKind`, `SourceLoc` and `Instance` are the public types of the snapshot. A package `Parameter` has `name`, `value` (the constant, or the alias as printed above; `logic[33:22]` for a plain vector) and `ty: Option<Type>`, set for aliases. Sandstitch's own `Parameter` (`src/import/package.rs`) keeps `slang-rs` types out of its API (decision 2): it stores the width of an alias's type in a private field at import, `calc_type_width` returns it, and `Parameter::new(name, value)` builds a parameter without one.

### `slang-rs`

`SlangOutput` holds the `design: Design`. `run_slang` returns it when the design elaborated without errors. `run_slang_keep_design` returns it whenever a compilation was created, with `ok` false after errors; every declared member is kept, and members whose types Slang could not resolve are unsupported. It returns the failure when no compilation was created (failed options, a missing source), so S0102 still reports Slang's text. `check_slang` stops after parsing and builds no design. `SlangConfig::signals` (default false) makes `run_slang` and `run_slang_keep_design` request `DesignQuery::WithSignals`, so that `Design::signals` answers. `extract_packages` runs Slang and returns `packages()`.

### Fixed behavior

The snapshot behaves as follows in these cases, each covered by its own test:

1. `UnsupportedPort::type_text` and `error` are Slang's display of the type and the reason.
1. A loop generate block's prefix uses the genvar value: `for (genvar i = 2; ...) begin : g` gives `g[2]`.
1. A typedef of a type parameter resolves to the parameter's type.
1. A `ref` port and a `MultiPortSymbol` port are unsupported ports; none panics or is dropped.
1. Instance array elements are part of the hierarchy.
1. A package alias of a struct, union or enum prints with its package-qualified name (`...}my_pkg::my_struct_t`), as port types are named.
1. A typedef that reaches a type parameter through aliases only (`typedef T tt;`, `typedef tt tt2;`) names the struct, union or enum the parameter resolves to, as any alias does (`c.tt`).
1. A value parameter whose type is a type parameter, or a typedef of one (`parameter T P = '0`), is listed by `parameters` with the type the type parameter resolves to.

### One path

Every query reads the snapshot. Sandstitch's callers (`src/import/typed.rs`, `src/import/specialize.rs`, `src/import/parser.rs`, `src/import/package.rs`, `src/import/hierarchy.rs`) take ports, parameters, packages and the hierarchy from the `Design` of `run_slang` or `run_slang_keep_design`.

### Testing

- **Generated designs** (`crates/slang-rs/tests/generated/`, `proptest`). A strategy generates a model of a package and a module hierarchy: ports and parameters of nested packed and unpacked structs and unions, enums (implicit, explicit, negative and wide values; signed and unsigned base types), packed and unpacked arrays with ascending, descending and negative ranges, typedefs in packages and modules, typedefs of arrays of named types, widths from parameters, type parameters with typedefs of them, child instances, instance arrays with ascending, descending and multi-dimensional ranges, and loop generate blocks with non-zero start values. A printer writes it as SystemVerilog. Every generated design must elaborate without diagnostics; a diagnostic fails the case as a generator defect. The test compares every query with the model, which encodes SystemVerilog's meaning, not either implementation; names of anonymous types are compared up to Slang's numeric id. CI runs proptest's default case count; `PROPTEST_CASES` raises it; failures shrink and are kept in `proptest-regressions/`.
- **Corpus** (`crates/slang-rs/tests/corpus.rs`). Three runs, one per library: `third_party/common_cells/src`, `third_party/axi/src` and `third_party/floonoc/hw`, without `deprecated/`, `test/`, `tb/`, `formal/` and `synth/`, each with its dependencies and include directories as in `designs/floonoc/src/lib.rs`. Every module of the library is passed as a `--top`, with `--allow-toplevel-iface-ports`, through `run_slang_keep_design`, and every query is answered for every top and instance. Tops defined in a file with an error diagnostic are not answered, and the test asserts that at least a recorded number of tops remain, so that a regression in options or sources cannot reduce it to checking error recovery. The test stays under 30 seconds in a debug build.
- The `slang-rs`, import, diagnostic and FlooNoC tests, which read the queries.
- A `slang-sys` unit test feeds tables with out-of-range and forward indices to the conversion and expects `Error`.

### Measurement

Import time and peak memory of the big FlooNoC 2x2 mesh (`cargo test --release -p floonoc --test bench -- --ignored --nocapture`, median of 3) are recorded on #18, against 457 ms elapsed and 97856 KiB own peak after step 1.

## Out of scope

- General Rust bindings for Slang.
- Linking a Slang library not built by this crate (decision 6).
- A command line tool; whether the explorer (#8) needs a small serving binary is open on #9.
- Python bindings; the static, position-independent library is the only preparation.
