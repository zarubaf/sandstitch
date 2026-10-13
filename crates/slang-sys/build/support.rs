// SPDX-License-Identifier: Apache-2.0

//! Pure helpers of the build script, unit tested through `src/lib.rs`.

use std::path::PathBuf;

/// A C++ compiler family with a known minimum version for C++20 Slang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilerId {
    Gcc,
    Clang,
    AppleClang,
}

/// The compiler family from the first line of its `--version` output.
pub fn identify(version_output: &str) -> Option<CompilerId> {
    let first = version_output.lines().next()?;
    if first.contains("Apple clang version") {
        Some(CompilerId::AppleClang)
    } else if first.contains("clang version") {
        Some(CompilerId::Clang)
    } else if first.contains("g++") || first.contains("gcc") || first.contains("(GCC)") {
        Some(CompilerId::Gcc)
    } else if first.starts_with("c++ (") {
        // Debian and Fedora name GCC's driver `c++`.
        Some(CompilerId::Gcc)
    } else {
        None
    }
}

/// The version number in a compiler's first `--version` line: the word
/// after `version` (Clang), else the last word (GCC).
fn compiler_version(first_line: &str) -> &str {
    first_line
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
        .or_else(|| first_line.split_whitespace().last())
        .unwrap_or("")
}

/// The leading number of `version` (`14` of `14.2.0`).
fn major(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

/// Ok if the compiler can build Slang: GCC 11, Clang 17 or Apple Clang 16
/// (Xcode 16) or newer.
pub fn check_compiler(id: CompilerId, version_output: &str) -> Result<(), String> {
    let first = version_output.lines().next().unwrap_or("");
    let version = compiler_version(first);
    let (minimum, name) = match id {
        CompilerId::Gcc => (11, "GCC 11"),
        CompilerId::Clang => (17, "Clang 17"),
        CompilerId::AppleClang => (16, "Apple Clang 16 (Xcode 16)"),
    };
    match major(version) {
        Some(found) if found >= minimum => Ok(()),
        _ => Err(format!(
            "slang-sys builds Slang, which needs C++20 with {name} or newer; found `{first}`"
        )),
    }
}

/// Ok if CMake is 3.20 or newer.
pub fn check_cmake(version_output: &str) -> Result<(), String> {
    let first = version_output.lines().next().unwrap_or("");
    let version = first.strip_prefix("cmake version ").unwrap_or("");
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    let found = (parts.next().flatten(), parts.next().flatten());
    match found {
        (Some(major), Some(minor)) if (major, minor) >= (3, 20) => Ok(()),
        _ => Err(format!(
            "slang-sys needs CMake 3.20 or newer to build Slang; found `{first}` \
             (`pip install cmake` installs a current one without root)"
        )),
    }
}

/// Ok if a Python 3 interpreter was found: Slang generates sources with it.
pub fn check_python(found: bool) -> Result<(), String> {
    if found {
        Ok(())
    } else {
        Err(
            "slang-sys needs Python 3 to build Slang (it generates sources); \
             none found on PATH"
                .to_string(),
        )
    }
}

/// The compiler launcher: ccache when found and not disabled.
pub fn launcher(path_lookup: Option<PathBuf>, no_ccache: bool) -> Option<PathBuf> {
    if no_ccache { None } else { path_lookup }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcc_versions() {
        let gcc = |out| check_compiler(CompilerId::Gcc, out);
        assert!(gcc("g++ (Debian 14.2.0-19) 14.2.0").is_ok());
        let e = gcc("g++ (GCC) 10.5.0").unwrap_err();
        assert!(e.contains("GCC 11") && e.contains("10.5.0"), "{e}");
    }

    #[test]
    fn clang_versions() {
        let clang = |out| check_compiler(CompilerId::Clang, out);
        let e = clang("clang version 16.0.6").unwrap_err();
        assert!(e.contains("Clang 17") && e.contains("16.0.6"), "{e}");
        assert!(clang("Ubuntu clang version 18.1.3 (1ubuntu1)").is_ok());
    }

    #[test]
    fn identifies_compilers() {
        assert_eq!(
            identify("g++ (Debian 14.2.0-19) 14.2.0\nCopyright (C) 2024"),
            Some(CompilerId::Gcc)
        );
        assert_eq!(
            identify("Ubuntu clang version 18.1.3 (1ubuntu1)\nTarget: x86_64"),
            Some(CompilerId::Clang)
        );
        let apple = "Apple clang version 16.0.0 (clang-1600.0.26.3)\nTarget: arm64";
        assert_eq!(identify(apple), Some(CompilerId::AppleClang));
        assert!(check_compiler(CompilerId::AppleClang, apple).is_ok());
        let old = "Apple clang version 15.0.0 (clang-1500.3.9.4)";
        let e = check_compiler(identify(old).unwrap(), old).unwrap_err();
        assert!(e.contains("Xcode 16"), "{e}");
        assert_eq!(identify("icpx (Intel) 2025.0"), None);
    }

    #[test]
    fn cmake_versions() {
        let e = check_cmake("cmake version 3.19.8\n\nCMake suite").unwrap_err();
        assert!(e.contains("3.20") && e.contains("pip install cmake"), "{e}");
        assert!(check_cmake("cmake version 4.4.4").is_ok());
        assert!(check_cmake("cmake version 3.20.0").is_ok());
    }

    #[test]
    fn python_is_required() {
        assert!(check_python(true).is_ok());
        assert!(check_python(false).unwrap_err().contains("Python 3"));
    }

    #[test]
    fn launcher_uses_ccache_unless_disabled() {
        let p = PathBuf::from("/usr/bin/ccache");
        assert_eq!(launcher(Some(p.clone()), false), Some(p.clone()));
        assert_eq!(launcher(Some(p), true), None);
        assert_eq!(launcher(None, false), None);
    }
}
