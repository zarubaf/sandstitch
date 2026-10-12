# Fork notes

Sandstitch is a hard fork of Topstitch.

- Upstream: https://github.com/xlsynth/topstitch
- Base: tag `v0.100.0`, commit `cf10fbcc0bfa17174e2cc55c7e4470fc2a72ed60`
- Forked: 2026-10-12

## Policy

- Compatible upstream fixes are taken with `git cherry-pick -x`, so the
  original commit is recorded in the message.
- Upstream paths stay at the repository root to keep cherry-picks applying.
- Divergences are listed below as they happen.

## Divergences from upstream

- `README.md` replaced.
- Removed `.github/workflows/publish.yml`, `tests/version_test.rs` and
  `check_version_is.py` (they depend on crates.io release state).
- `Cargo.toml`: `publish = false`; `repository` and `homepage` point to
  https://github.com/zarubaf/sandstitch; `documentation` removed; unused
  dev-dependencies removed (`curl`, `semver`, `toml`, `serde_json`).
