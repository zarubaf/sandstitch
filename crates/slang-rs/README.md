# slang-rs

Parse SystemVerilog with Slang using a Rust API.

Note: the API is currently under development and is subject to frequent changes.

## Slang version compatibility

Slang v11.0 is vendored and linked in-process through the `slang-sys` crate.
No other Slang version is supported.

## Prerequisite

Slang v11.0 is built from the vendored source by the `slang-sys` crate and
called in-process, so no `slang` binary and no `SLANG_PATH` are needed. Building
needs GCC 11+ or Clang 17+, CMake 3.20+ and Python 3; `ccache`, if installed, speeds up
rebuilds.

## Installation

Install Rust 1.85.0 or newer using the authenticated installation method for
your platform. CI currently pins Rust 1.85.0, the first release supporting the
Rust 2024 edition.

Then clone this repository:

```shell
git clone https://github.com/xlsynth/slang-rs.git
```

To run the tests:

```shell
cd slang-rs
```

```shell
cargo test
```

## Development

We use [pre-commit](https://pre-commit.com/) as part of our CI pipeline.

The development lock file targets Python 3.12. Create and activate a Python
3.12 virtual environment, then install the hash-locked tools:

```shell
python3.12 -m venv .venv
source .venv/bin/activate
python -m pip install --require-hashes -r requirements-ci.txt
```

Then install the pre-commit hooks for this repository with:

```shell
pre-commit install
```

The pre-commit hooks will run automatically when you attempt to commit code. You can also run pre-commit checks on-demand with:

```shell
pre-commit run
```
