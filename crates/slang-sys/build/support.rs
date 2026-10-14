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

/// The first `\d+\.\d+(\.\d+)?` token of `text`, or "".
fn first_version(text: &str) -> &str {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|t| {
            let parts: Vec<&str> = t.split('.').collect();
            (2..=3).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty())
        })
        .unwrap_or("")
}

/// The version number in a compiler's first `--version` line: the first
/// version-shaped token after the word `version` (Clang), else after the
/// closing parenthesis of the vendor text (GCC).
fn compiler_version(first_line: &str) -> &str {
    if let Some(i) = first_line.find("version ") {
        return first_version(&first_line[i + "version ".len()..]);
    }
    match first_line.find(')') {
        Some(i) => first_version(&first_line[i + 1..]),
        None => first_version(first_line),
    }
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
            "slang-sys builds Slang, which needs C++20 with {name} or newer; found `{first}` \
             (run inside `nix develop`)"
        )),
    }
}

/// Ok if CMake is 3.28 or newer.
pub fn check_cmake(version_output: &str) -> Result<(), String> {
    let first = version_output.lines().next().unwrap_or("");
    let version = first.strip_prefix("cmake version ").unwrap_or("");
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    let found = (parts.next().flatten(), parts.next().flatten());
    match found {
        (Some(major), Some(minor)) if (major, minor) >= (3, 28) => Ok(()),
        _ => Err(format!(
            "slang-sys needs CMake 3.28 or newer to build Slang; found `{first}` \
             (run inside `nix develop`)"
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
             none found on PATH (run inside `nix develop`)"
                .to_string(),
        )
    }
}

/// The compiler launcher: ccache when found and not disabled.
pub fn launcher(path_lookup: Option<PathBuf>, no_ccache: bool) -> Option<PathBuf> {
    if no_ccache { None } else { path_lookup }
}

/// The value of `CMAKE_{C,CXX}_COMPILER_LAUNCHER`: always defined, so an
/// empty value clears a launcher cached in `CMakeCache.txt`.
pub fn launcher_value(launcher: Option<&PathBuf>) -> String {
    launcher
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// The generator recorded in `CMakeCache.txt` text.
pub fn cached_generator(cache: &str) -> Option<String> {
    cache
        .lines()
        .find_map(|l| l.strip_prefix("CMAKE_GENERATOR:INTERNAL="))
        .map(str::to_string)
}

/// The generator to configure with: the one an existing build directory
/// recorded (CMake rejects a change), else Ninja if present, else CMake's
/// default (`None`).
pub fn generator(cache: Option<&str>, ninja_present: bool) -> Option<String> {
    cache
        .and_then(cached_generator)
        .or_else(|| ninja_present.then(|| "Ninja".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcc_versions() {
        let gcc = |out| check_compiler(CompilerId::Gcc, out);
        assert!(gcc("g++ (Debian 14.2.0-19) 14.2.0").is_ok());
        let e = gcc("g++ (GCC) 10.5.0").unwrap_err();
        assert!(
            e.contains("GCC 11") && e.contains("10.5.0") && e.contains("nix develop"),
            "{e}"
        );
    }

    #[test]
    fn gcc_version_is_the_number_after_the_vendor_text() {
        let gcc = |out| check_compiler(CompilerId::Gcc, out);
        // A revision hash in brackets after the version.
        assert!(gcc("gcc (SUSE Linux) 14.2.1 20250220 [revision 0bc9abc]").is_ok());
        // A parenthesised suffix after the version.
        assert!(gcc("g++ (GCC) 16.0.0 20250601 (experimental)").is_ok());
        // The date must not be read as the major version.
        assert!(gcc("g++ (GCC) 10.2.0 20200723").is_err());
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
        assert!(e.contains("3.28") && e.contains("nix develop"), "{e}");
        assert!(check_cmake("cmake version 4.4.4").is_ok());
        assert!(check_cmake("cmake version 3.28.0").is_ok());
    }

    #[test]
    fn python_is_required() {
        assert!(check_python(true).is_ok());
        let e = check_python(false).unwrap_err();
        assert!(e.contains("Python 3") && e.contains("nix develop"), "{e}");
    }

    #[test]
    fn launcher_uses_ccache_unless_disabled() {
        let p = PathBuf::from("/usr/bin/ccache");
        assert_eq!(launcher(Some(p.clone()), false), Some(p.clone()));
        assert_eq!(launcher(Some(p), true), None);
        assert_eq!(launcher(None, false), None);
    }

    #[test]
    fn launcher_value_is_empty_when_off() {
        // An empty value overrides a launcher cached by an earlier configure.
        assert_eq!(launcher_value(None), "");
        let p = PathBuf::from("/usr/bin/ccache");
        assert_eq!(launcher_value(Some(&p)), "/usr/bin/ccache");
    }

    #[test]
    fn generator_comes_from_the_cache_or_ninja() {
        let cache = "# comment\nCMAKE_BUILD_TYPE:STRING=Release\n\
                     CMAKE_GENERATOR:INTERNAL=Unix Makefiles\nOTHER:INTERNAL=x\n";
        assert_eq!(cached_generator(cache).as_deref(), Some("Unix Makefiles"));
        assert_eq!(cached_generator("CMAKE_BUILD_TYPE:STRING=Release\n"), None);
        // A recorded generator wins over Ninja appearing on PATH.
        assert_eq!(
            generator(Some(cache), true).as_deref(),
            Some("Unix Makefiles")
        );
        // A fresh build directory takes Ninja if present, else the default.
        assert_eq!(generator(None, true).as_deref(), Some("Ninja"));
        assert_eq!(generator(None, false), None);
    }
}
