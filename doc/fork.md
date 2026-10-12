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
