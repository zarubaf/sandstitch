# AGENTS.md

## Dependency provenance

Keep every third-party input used by this repository reproducible and tied to
immutable provenance.

- Build and test inside the repository's `nix develop`, pinned by `flake.lock`,
  and run `cargo xtask ci` from the repository root. Do not install toolchains
  by other means.
- Pin GitHub Actions to full 40-character commit SHAs, with a trailing comment
  naming the release tag.
- Commit `Cargo.lock`, retain registry checksums, and use `cargo --locked` in CI.
  Normal version requirements remain in `Cargo.toml` for downstream library
  compatibility.
