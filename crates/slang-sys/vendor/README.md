# Vendored sources

Unmodified subsets of two upstream releases, built by `../build.rs`.

| Project | Version | Source | sha256 of the tarball |
| --- | --- | --- | --- |
| [Slang](https://github.com/MikePopoloski/slang) | v11.0 | https://github.com/MikePopoloski/slang/archive/refs/tags/v11.0.tar.gz | `50676d5a9adbefb97d266a4b174e6b0513901afd5ac57a6cdfea0a61149c3704` |
| [fmt](https://github.com/fmtlib/fmt) | 12.1.0 | https://github.com/fmtlib/fmt/archive/refs/tags/12.1.0.tar.gz | `ea7de4299689e12b6dddd392f9896f08fb0777ac7168897a244a6d6085043fea` |

Kept from Slang: `CMakeLists.txt`, `cmake/`, `external/`, `include/`, `scripts/` (source generators run at build time), `source/`, `LICENSE`, `LICENSES/`, `REUSE.toml`. Dropped: `bindings/`, `docs/`, `pyslang/`, `tests/`, `tools/` and the top-level project files.

Kept from fmt: `CMakeLists.txt`, `include/`, `src/`, `support/cmake/`, `LICENSE`, `README.md` and `ChangeLog.md` (fmt's CMake lists the last two as module sources). Dropped: `doc/`, `test/`, the rest of `support/` and `CONTRIBUTING.md`.

To update, download the new tarball into an empty directory, check its sha256, copy the same subsets over the old ones, update this table and the `release!` macro in `../src/lib.rs`, and record the change in `doc/fork.md`.
