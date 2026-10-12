# Fork notes

Sandstitch is a hard fork of Topstitch.

- Upstream: https://github.com/xlsynth/topstitch
- Base: tag `v0.100.0`, commit `cf10fbcc0bfa17174e2cc55c7e4470fc2a72ed60`
- Forked: 2026-10-12

## Policy

- Port upstream fixes by hand and reference the upstream commit
  (`xlsynth/topstitch@<sha>`) in the commit body.
- Upstream tag merges no longer apply: `src/` is organised by concern (see
  Layout), so the paths have diverged. The base above stays recorded.
- Divergences are listed below as they happen.

## Layout

Upstream path (base `v0.100.0`) to Sandstitch path:

| Upstream | Sandstitch |
| --- | --- |
| `src/mod_def.rs` | `src/model/mod_def.rs` |
| `src/mod_def/{core,feedthrough,hierarchy,instances,intf,ports,stub,wrap}.rs` | `src/model/mod_def/` |
| `src/mod_inst.rs`, `src/funnel.rs`, `src/pipeline.rs`, `src/io.rs`, `src/usage.rs`, `src/metadata.rs` | `src/model/` |
| `src/port.rs` + `src/port/`, `src/port_slice.rs` + `src/port_slice/`, `src/intf.rs` + `src/intf/` | `src/model/` |
| `src/connection.rs` + `src/connection/{connected_item,expression_source,port_slice}.rs` | `src/model/` |
| `src/connection/validate.rs` | `src/check/connection.rs` |
| `src/mod_def/validate.rs` | `src/check/mod_def.rs` |
| `src/mod_def/{parser,parser_cfg,parameterize}.rs`, `src/package.rs` | `src/import/` |
| `src/mod_def/emit.rs` | `src/emit/verilog.rs` |
| `src/mod_def/dtypes.rs` | `src/physical/geometry.rs` |
| `src/mod_def/lefdef.rs` | `src/physical/lefdef_io.rs` |
| `src/lefdef.rs` | `src/physical/lefdef.rs` (still public as `topstitch::lefdef`) |
| `src/mod_def/{def_parse,lef_parse,pins,tracks,placement,shape,edges}.rs` | `src/physical/` |
| `src/validate.rs` + `src/validate/` | `src/physical/validate.rs` + `src/physical/validate/` |

Items also moved between files: the physical methods of `ModDef`, `ModInst`,
`Port`, `PortSlice`, `Intf` and `IO` now live in
`src/physical/{mod_def,mod_inst,port,port_slice,intf,io}.rs`; `ParameterType`,
`ParameterSpec` and `ParameterValue` are in `src/model/parameter.rs`;
`VerilogImport` is in `src/import/parser.rs`; `populate_hierarchy` is in
`src/import/hierarchy.rs`.

`src/lib.rs`, `src/util.rs` and `src/diagnostic/` keep their paths. The public
API (`topstitch::*`, `topstitch::lefdef`, `topstitch::diagnostic`) is
unchanged; `src/lib.rs` re-exports it. `git log --follow` traces a file
across the move.

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
  `tests/syntax/params.rs` (upstream paths; see Layout for the current ones).
- `src/` reorganised by concern (see Layout); the moves changed only module,
  `use` and macro paths.
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
  (`CARGO_MANIFEST_DIR`) instead of the whole workspace root;
  `tests/version_test.rs` is deleted (it compared the root package version with
  crates.io and needed network) together with the dev-dependencies only it
  used (`curl`, `toml`, `semver`, `cargo_metadata`, `log`, `env_logger`);
  `[lints.clippy]` allows `collapsible_if`, `unneeded_wildcard_pattern` and
  `while_let_loop` for pre-existing upstream code.
- Sandstitch extensions:
  - `feat(slang-rs): resolve type parameters, add locations and interface ports`.
  - `feat(slang-rs): parse integral atom types, keep enum and struct signedness
    and list unparsable ports`: the grammar accepts `byte`, `shortint`,
    `integer`, `time` and `struct|union packed signed|unsigned`;
    `Type::Struct::signed`, `Type::Union::signed` and `Variant::signed`;
    `is_aggregate` recognizes signed aggregates, so a named signed struct
    resolves to its typedef name (`sp::s_t`, not `sp::s$3`); `UnsupportedPort`
    and `extract_instance_ports_checked_from_value`; `absolute_path` is public.

### xlsynth-vast (vendored)

- Upstream: https://github.com/xlsynth/xlsynth-crate (crate `xlsynth-vast`)
- Base: crates.io `0.65.0`, monorepo tag `v0.65.0`,
  commit `b7cdab13d1f316e855d6075144998e12c28bdefe`
- Vendored from the crates.io tarball (the upstream monorepo is not
  subtree-friendly). The normalized `Cargo.toml` is kept, because the original
  uses `lints.workspace = true`, which has no counterpart here.
  `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` are dropped.
- Sync: do not replace the directory (that would wipe local patches). Download
  the old and new published crates, `diff -ruN old new`, and apply the diff
  with `patch -p1` inside `crates/xlsynth-vast/` (or re-apply the commits from
  `git log -- crates/xlsynth-vast`), then update the base above.
- Local divergences:
  - Manifest: the `[[test]]` list, `autotests`, `autolib`, `autobins` and
    `resolver = "2"` are removed (so new test files run and the workspace
    resolver applies); `[lints.clippy] write_with_newline` and `clone_on_copy`
    are allowed for pre-existing upstream code.
  - Sandstitch extensions (`feat(xlsynth-vast): type expressions, package refs
    and member selects`): `make_type_expr`, `make_package_ref`,
    `make_member_select`, `make_member_select_indexable` and
    `tests/sandstitch_ext_test.rs`.
- Upstreaming: send patches against the xlsynth-crate monorepo.

### Third-party RTL

`third_party/` holds squashed subtrees of FlooNoC, common_cells and axi for
tests only; see `third_party/README.md`. `third_party/` and `crates/slang-rs/` keep
their upstream licence headers and are excluded from the root SPDX test (`crates/xlsynth-vast/` is checked).
