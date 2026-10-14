# Compatibility policy

## Versions

All workspace crates carry one SemVer version, set in `[workspace.package]` in the root `Cargo.toml` (see [fork.md](fork.md), "Releases"). Before 1.0:

- a **minor** release (`0.N.0`) may break anything listed under "Covered";
- a **patch** release (`0.N.P`) does not: it fixes defects and adds backward-compatible functionality only;
- pre-releases (`0.N.0-alpha.K`) carry no promise.

Breaking changes are listed in the release notes.

## Covered

- **The public Rust API of `topstitch`**: the items reachable from the crate root.- **Diagnostic codes** (`S0xxx`) are stable identifiers: a code keeps its meaning, and a retired code is not reused. The message text, labels, notes and help may change in any release.
- **Emitted RTL** of a valid design changes only in minor releases. Patch releases may change it only to fix output that was wrong, such as invalid SystemVerilog or a connection that did not match the model.
- **The manifest** (`Manifest` and its fields) keeps its fields and their meaning within a minor series. Fields may be added in minor releases.

## Not covered

- Internals: items that are not public, the module layout behind the crate root, and the order of unordered collections.
- The APIs of the vendored crates (`slang-rs`, `slang-sys`, `xlsynth-vast`); they are implementation details of `topstitch`.
- The Slang version. It is pinned per release (the vendored source) and listed in the release notes; a different version can change which SystemVerilog is accepted and the diagnostics from Slang (S0102).
- The designs under `designs/` and the examples, which track the library.
- Performance, apart from the budgets of the reference designs in [doc/design/reference-noc.md](design/reference-noc.md), which hold on the recorded machine.

## Supported environment

Building needs stable Rust (each release is tested with the pinned toolchain; v0.1.0 with 1.98.1), a C++20 compiler (GCC 11+ or Clang 17+), CMake 3.28+ and Python 3. `build.rs` checks the C++ compiler, CMake and Python; x86-64 Linux is the supported target. Each release is tested in `nix develop` from its `flake.lock`, which is the reference toolchain when a build elsewhere behaves differently. Downstream projects depend on a release by tag through Cargo's git source; see the [guide](guide.md).
