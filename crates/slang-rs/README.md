# slang-rs

Parse SystemVerilog with Slang using a Rust API.

Note: the API is currently under development and is subject to frequent changes.

## Slang version compatibility

Slang v12.0 is vendored and linked in-process through the `slang-sys` crate.
No other Slang version is supported.

## Prerequisite

Slang v12.0 is built from the vendored source by the `slang-sys` crate and
called in-process, so no `slang` binary and no `SLANG_PATH` are needed. `build.rs`
checks for GCC 11+ or Clang 17+, CMake 3.28+ and Python 3; any toolchain
meeting these floors works, and the repository's `nix develop` is the
development environment.

## Development

The build environment is the repository's `nix develop` (`flake.nix`, pinned by
`flake.lock`), which provides Rust, a C++ compiler, CMake, Python 3, ccache and
Verilator. Inside it, run the checks from the repository root:

```shell
nix develop --command cargo xtask ci
```
