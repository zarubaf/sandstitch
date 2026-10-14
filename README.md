# Sandstitch

Sandstitch is a Rust library for building hardware modules and complete SoCs from existing SystemVerilog IP and generated components. A design is ordinary Rust code: it imports IP, instantiates it, connects it, checks the result, and emits integration SystemVerilog plus a build manifest, along with LEF/DEF views for physical integration.

## What it addresses

- **Hand-written integration RTL.** Top levels, wrappers and meshes, and the SystemVerilog packages they configure (`PackageDef`), are generated from the model. Only the imported IP is hand-written.
- **Wiring mistakes found late.** Undriven, unconnected or multiply driven bits, type mismatches and net name clashes are reported as coded diagnostics that point at the Rust line that made the connection. Width mismatches are rejected at the `connect` call itself.
- **Parameterized IP.** Slang runs in-process and elaborates each configuration, so ports keep their real widths and struct, union and enum types, and several specializations of one module coexist in one design.
- **Reproducible builds.** The manifest lists exactly the sources, include directories and packages the emitted RTL needs. Development and CI use a toolchain pinned with Nix.

## Using it

Add the library as a git dependency (the package is named `topstitch`):

```toml
topstitch = { git = "https://github.com/zarubaf/sandstitch", tag = "v0.1.0" }
```

Sandstitch uses [Slang](https://github.com/MikePopoloski/slang), a C++ SystemVerilog compiler, to import IP. Slang is vendored and compiled from source on the first build (minutes). Building therefore needs stable Rust (each release is tested with the pinned toolchain; v0.1.0 with 1.98.1), a C++20 compiler (GCC 11+ or Clang 17+), CMake 3.28+ and Python 3. `build.rs` checks the C++ compiler, CMake and Python and says what is missing. `nix develop github:zarubaf/sandstitch/v0.1.0` is one way to get exactly the toolchain the release was tested with.

```rust
use topstitch::{EmitOptions, ModDef};

let adder = ModDef::from_verilog_file("adder", "adder.sv".as_ref(), false, false);
let top = ModDef::new("top");
let first = top.instantiate(&adder, Some("first"), None);
let second = top.instantiate(&adder.parameterize(&[("W", 9)]), Some("second"), None);
first.get_port("sum").connect(&second.get_port("a"));
// ... export the remaining ports ...

let diagnostics = top.check(); // wiring problems, with codes and locations
let rtl = top.emit(EmitOptions::default());
let sources = top.manifest().sources; // what the RTL needs to build
```

Where to go next:
- [doc/guide.md](doc/guide.md) walks through this example; the full code is in [examples/getting_started.rs](examples/getting_started.rs).
- [designs/floonoc](designs/floonoc) is a complete reference design: a FlooNoC mesh with compute and memory tiles, simulated in Verilator.
- [doc/errors](doc/errors/README.md) explains each diagnostic, and `cargo xtask explain <CODE>` shows the same text.
- [doc/compatibility.md](doc/compatibility.md) says what a version promises, and [doc/product.md](doc/product.md) is the product plan.

## Building

To work on Sandstitch itself, `nix develop` enters the pinned development environment (`flake.nix`, `flake.lock`) on x86-64 Linux. It provides Rust, a C++ compiler, CMake, Python 3, ccache and Verilator, and CI uses the same environment. Slang is vendored and runs in-process. The first build compiles it (minutes); ccache makes rebuilds instant. `cargo xtask ci` runs the repository checks.

## Relation to Topstitch

Sandstitch is a hard fork of [Topstitch](https://github.com/xlsynth/topstitch) v0.100.0 (Apache-2.0, copyright its authors), taken on 2026-10-12. Topstitch's composition model and physical-design API, and its VAST emitter, are the base and are kept. The fork exists because the product needs changes that go deeper than upstream patches:
- Slang runs in-process from vendored source, instead of an external `slang` binary and its JSON output.
- Imported ports keep their struct, union and enum types, and specializations of one module coexist.
- Connections go through one resolver shared by checking and emission, and errors are structured, coded diagnostics (`check`, `try_*`) instead of plain panic messages.
- The emitted RTL comes with packages built from the model and a build manifest.
- The public API is a smaller, explicit surface.

`src/` is reorganised by concern, so upstream merges no longer apply. Upstream fixes are ported by hand. [doc/fork.md](doc/fork.md) records the base, the policy and every divergence.
