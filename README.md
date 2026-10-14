# Sandstitch

Sandstitch is a reusable library for constructing hardware modules and complete SoCs from existing RTL/IP and generated components. Running a design package resolves a selected configuration and produces integration SystemVerilog, an interactive structural explorer, and module documentation from the same executable design.

See [doc/product.md](doc/product.md) for the product plan.

Imported SystemVerilog modules can be specialized with value and type parameters (`try_parameterize`, or `try_from_verilog_parameterized` for IP whose defaults do not elaborate). Ports keep their resolved struct, union and enum types, connections between them are type-checked, and the emitted RTL uses typed nets where the type can be named, plus a build manifest of the sources it needs. See [doc/design/typed-specialization.md](doc/design/typed-specialization.md); [designs/floonoc](designs/floonoc) specializes the FlooNoC router and builds a mesh of compute and memory tiles with it, as a small and a large system with a Verilator traffic harness. Errors are documented in [doc/errors](doc/errors/README.md).

## Building

On x86-64 Linux, `nix develop` enters the pinned development environment (`flake.nix`, `flake.lock`): Rust, a C++ compiler, CMake, Python 3, ccache and Verilator. CI uses the same environment.

Slang is vendored and runs in-process. The first build compiles it (minutes); ccache makes rebuilds instant. `cargo xtask ci` runs the repository checks.

Forked from [Topstitch](https://github.com/xlsynth/topstitch) v0.100.0 (Apache-2.0), copyright its authors.
