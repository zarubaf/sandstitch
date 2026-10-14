# Getting started

Sandstitch is a Rust library: a design is a Rust program that imports SystemVerilog modules, instantiates and connects them, checks the result and emits SystemVerilog. The library crate is named `topstitch`. The supported environment is `nix develop`; see the [compatibility policy](compatibility.md).

## Environment

From your own project, enter the environment pinned by the release:

```sh
nix develop github:zarubaf/sandstitch/v0.1.0
```

Flakes must be enabled: either pass `--extra-experimental-features 'nix-command flakes'` to `nix`, or set `experimental-features = nix-command flakes` in `nix.conf`.

To work on Sandstitch itself, clone the repository:

```sh
git clone https://github.com/zarubaf/sandstitch
cd sandstitch
nix develop
```

The shell provides Rust, a C++ compiler, CMake, Python 3, ccache and Verilator, pinned by `flake.lock`. Slang is vendored and linked in-process. The first build compiles it (minutes); ccache makes later builds instant.

## Depending on the library

The crates are not published to crates.io. Depend on a tag through Cargo's git source, inside `nix develop`:

```toml
[dependencies]
topstitch = { git = "https://github.com/zarubaf/sandstitch", tag = "v0.1.0" }
```

Use the package name `topstitch`; the repository is named Sandstitch. Pin the Nix environment of the same tag (its `flake.lock`) for your own builds.

## Walk-through

`cargo run --example getting_started` runs [examples/getting_started.rs](../examples/getting_started.rs), which `tests/examples.rs` also runs in CI. Its steps:

1. **Import** a module from a SystemVerilog file ([examples/input/adder.sv](../examples/input/adder.sv)) and specialize a parameter:

   ```rust
   let adder = ModDef::from_verilog_file("adder", &file, false, false);
   let adder_9_bit = adder.parameterize(&[("W", 9)]);
   ```

   Slang parses and elaborates the file. The third argument is `ignore_unknown_modules`; `adder.sv` instantiates no submodules, so it is `false`. These forms panic with the rendered diagnostics on failure; `try_parameterize` and `try_from_verilog_with_config` return them as an `ImportError` instead.
2. **Instantiate** the modules into a new `ModDef`:

   ```rust
   let top = ModDef::new("top");
   let first = top.instantiate(&adder, Some("first"), None);
   let second = top.instantiate(&adder_9_bit, Some("second"), None);
   ```
3. **Export** ports of instances as ports of `top`, and **connect** instance ports:

   ```rust
   first.get_port("a").export_as("x");
   first.get_port("sum").connect(&second.get_port("a"));
   ```
4. **Check** the design. `check()` returns `Diagnostics`; each has a code, a severity and source spans:

   ```rust
   let diagnostics = top.check();
   assert!(!diagnostics.has_errors(), "{}", diagnostics.render());
   ```
5. **Emit** the SystemVerilog and read the **manifest**, which lists the sources, include directories, libraries, defines and packages the emitted RTL needs and the specialization of each specialized instance:

   ```rust
   let rtl = top.emit(EmitOptions::default());
   let sources = top.manifest().sources;
   ```

   The emitted `top` instantiates `adder` and `adder #(.W(32'h0000_0009))`, and the manifest lists `adder.sv` as the source that defines them.

## Where to go next

- **Diagnostics.** Every error and warning has a stable code `S0xxx`. `cargo xtask explain <CODE>` shows its documentation, with an example and the fix; the entries live in [doc/errors](errors/README.md).
- **Packages.** `PackageDef` builds SystemVerilog packages in Rust and hands out typed handles for parameter overrides: [doc/design/packages.md](design/packages.md).
- **Connections.** Which connections are legal, and what `check()` reports about them: [doc/design/connection-contract.md](design/connection-contract.md). Typed ports and specialization: [doc/design/typed-specialization.md](design/typed-specialization.md). Bundled interfaces: [doc/design/bundled-interfaces.md](design/bundled-interfaces.md).
- **Physical design.** Import and emit LEF/DEF (`ModDef::from_lef_file`, `from_def_file`, `emit_lef`), place instances, define pins and tracks. See the root re-exports `lefdef` (with `lefdef::generate_lef`), `LefDefOptions`, `MultiplePinShapesPolicy`, `Placement`, `TrackDefinitions` and `SpreadPinsOptions`, and `examples/pinning.rs`.
- **A complete design.** [designs/floonoc](../designs/floonoc) is the reference design: a router, a compute and memory tile and a mesh, with a Verilator traffic harness. [doc/design/reference-noc.md](design/reference-noc.md) describes it and its measured budgets.
- **Compatibility.** What the version number promises: [compatibility.md](compatibility.md).
