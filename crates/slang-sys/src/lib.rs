// SPDX-License-Identifier: Apache-2.0

//! Slang, built from the vendored source in `vendor/` and called in-process
//! through a narrow C++ shim (`shim/`).

/// The vendored Slang release.
pub const SLANG_VERSION: &str = "11.0";

#[cxx::bridge(namespace = "slang_sys")]
mod ffi {
    unsafe extern "C++" {
        include!("slang-sys/shim/shim.h");

        fn library_version() -> String;
    }
}

/// `major.minor.patch` of the linked Slang library, for example `11.0.0`.
pub fn library_version() -> String {
    ffi::library_version()
}

#[cfg(test)]
#[path = "../build/support.rs"]
#[allow(dead_code)]
mod support;
