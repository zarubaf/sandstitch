# Fork notes

Sandstitch is a hard fork of Topstitch.

- Upstream: https://github.com/xlsynth/topstitch
- Base: tag `v0.100.0`, commit `cf10fbcc0bfa17174e2cc55c7e4470fc2a72ed60`
- Forked: 2026-10-12

## Policy

- Compatible upstream fixes are taken with `git cherry-pick -x`, so the
  original commit is recorded in the message.
- Larger syncs merge an upstream tag (`git merge <tag>`), the one allowed
  exception to linear history. Update the base above when doing so.
- Upstream paths stay at the repository root to keep cherry-picks and merges
  applying.
- Divergences are listed below as they happen.

## Divergences from upstream

- `README.md` replaced.
- Removed `.github/workflows/publish.yml`, `tests/version_test.rs` and
  `check_version_is.py` (they depend on crates.io release state).
- `Cargo.toml`: `publish = false`; `repository` and `homepage` point to
  https://github.com/zarubaf/sandstitch; `documentation` removed; unused
  dev-dependencies removed (`curl`, `semver`, `toml`, `serde_json`).
- `.github/workflows/ci.yml`: the clippy step runs
  `cargo clippy --all-targets --locked -- -D warnings` and gates the build
  (upstream tolerated warnings).
- Clippy lint fixes (behavior-preserving: redundant borrows and useless
  `format!` removed) in `src/mod_def/emit.rs`, `src/mod_def/placement.rs`,
  `src/mod_def/lefdef.rs`, `src/mod_def/hierarchy.rs` and
  `tests/syntax/params.rs`.
- `Cargo.toml`: `slang-rs = "=0.22.0"` and `xlsynth-vast = "=0.65.0"` are
  pinned exactly (upstream used caret requirements).

## Forked dependencies

Both live under `crates/` as workspace members; the library depends on them by
path and keeps the exact version requirements (`=0.22.0`, `=0.65.0`).

### slang-rs (subtree)

- Upstream: https://github.com/xlsynth/slang-rs.git
- Base: tag `v0.22.0`, commit `89906f2f851d526c404671e6d9a7f2aabb5ebe94`
- Imported with `git subtree add --prefix crates/slang-rs` (full history, not
  squashed), so the import is a merge commit.
- Sync: `git subtree pull --prefix crates/slang-rs https://github.com/xlsynth/slang-rs.git <tag>`
  and update the base above.
- Upstreaming: `git subtree push --prefix crates/slang-rs <fork-url> <branch>`
  or `git format-patch` on commits touching `crates/slang-rs/`.
- Divergences: `tests/spdx_test.rs` scans only the crate directory
  (`CARGO_MANIFEST_DIR`) instead of the whole workspace root.

### xlsynth-vast (vendored)

- Upstream: https://github.com/xlsynth/xlsynth-crate (crate `xlsynth-vast`)
- Base: crates.io `0.65.0`, monorepo tag `v0.65.0`,
  commit `b7cdab13d1f316e855d6075144998e12c28bdefe`
- Vendored from the crates.io tarball (the upstream monorepo is not
  subtree-friendly). The normalized `Cargo.toml` is kept, because the original
  uses `lints.workspace = true`, which has no counterpart here.
  `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` are dropped.
- Sync: download the new crate version, replace `crates/xlsynth-vast/`
  wholesale, and update the base above.
- Upstreaming: send patches against the xlsynth-crate monorepo.

### Third-party RTL

`third_party/` holds squashed subtrees of FlooNoC, common_cells and axi for
tests only; see `third_party/README.md`. `crates/` and `third_party/` keep
their upstream licence headers and are excluded from the root SPDX test.
