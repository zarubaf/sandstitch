# Vendored sources

Unmodified subsets of four upstream releases, built by `../build.rs`. Slang 12 fetches fmt, toml++ and boost::regex with CMake `FetchContent`; the build points `FETCHCONTENT_SOURCE_DIR_*` at these copies, so it needs no network. Each is the exact tag Slang v12.0 pins.

| Project | Version | Source | sha256 of the tarball |
| --- | --- | --- | --- |
| [Slang](https://github.com/MikePopoloski/slang) | v12.0 | https://github.com/MikePopoloski/slang/archive/refs/tags/v12.0.tar.gz | `64b3eb9d38ee126e009cbb8da0cfa6f68d970334e52ba084ad7c68e4b5fa804c` |
| [fmt](https://github.com/fmtlib/fmt) | 12.2.0 | https://github.com/fmtlib/fmt/archive/refs/tags/12.2.0.tar.gz | `8b852bb5aa6e7d8564f9e81394055395dd1d1936d38dfd3a17792a02bebd7af0` |
| [toml++](https://github.com/marzer/tomlplusplus) | v3.4.0 | https://github.com/marzer/tomlplusplus/archive/refs/tags/v3.4.0.tar.gz | `8517f65938a4faae9ccf8ebb36631a38c1cadfb5efa85d9a72e15b9e97d25155` |
| [boost::regex](https://github.com/MikePopoloski/regex) (header-only fork) | boost-1.91.0 | https://github.com/MikePopoloski/regex/archive/refs/tags/boost-1.91.0.tar.gz | `b159cea3bf570394dd19f5fed7320005290809ba53fea15c630c3a6c2685db75` |

Kept from Slang: `CMakeLists.txt`, `cmake/`, `external/`, `include/`, `scripts/` (source generators run at build time), `source/`, `LICENSE`, `LICENSES/`, `REUSE.toml`. Dropped: `bindings/`, `docs/`, `examples/`, `pyslang/`, `tests/`, `tools/` and the top-level project files.

Kept from fmt: `CMakeLists.txt`, `include/`, `src/`, `support/cmake/`, `LICENSE`, `README.md` and `ChangeLog.md` (fmt's CMake lists the last two as module sources). Dropped: `doc/`, `test/`, the rest of `support/` and `CONTRIBUTING.md`. Slang compiles the fmt implementation itself (`source/util/Fmt.cpp`), so fmt's own library target is not built or linked.

Kept from toml++: `CMakeLists.txt`, `cmake/`, `include/` and `LICENSE`. Dropped: everything else.

Kept from boost::regex: `include/`, `README.md` and `readme.txt`. The tarball has no license file; every header carries the Boost Software License 1.0 notice and refers to `LICENSE_1_0.txt`; the license text is `crates/slang-sys/vendor/slang/LICENSES/BSL-1.0.txt`. Dropped: everything else (Slang adds the repository as a source only, `SOURCE_SUBDIR _no_build_`, so its CMake file is never read).

To update, download each new tarball into its own empty directory, check its sha256, copy the same subsets over the old ones, update this table and the `release!` macro in `../build/release.rs`, and record the change in `doc/fork.md`. The tags to vendor are the `GIT_TAG` entries in Slang's `external/CMakeLists.txt`; check them and the CMake minimum (`cmake_minimum_required` in Slang's `CMakeLists.txt`) on every Slang update.
