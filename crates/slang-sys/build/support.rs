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

/// The version number in a compiler's `--version` output, or "".
pub fn version(version_output: &str) -> &str {
    compiler_version(version_output.lines().next().unwrap_or(""))
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

/// The C++ standard Slang and the shim are compiled with.
pub const CXX_STD: &str = "c++20";

/// Whether Slang is built as position-independent code (Python extension
/// modules later); recorded as `pic` in the manifest.
pub const PIC: bool = true;

/// Slang's assertion setting, for the library and the shim alike.
pub const ASSERT_DEFINE: &str = "SLANG_ASSERT_ENABLED=1";

/// The defines the shim is compiled with, as `NAME` or `NAME=VALUE`; they
/// mirror the settings the library was built with.
pub const SHIM_DEFINES: [&str; 4] = [
    "SLANG_STATIC_DEFINE",
    "SLANG_BOOST_SINGLE_HEADER",
    "SLANG_USE_THREADS",
    ASSERT_DEFINE,
];

/// A define as the name and the optional value `cc::Build::define` takes.
pub fn split_define(define: &str) -> (&str, Option<&str>) {
    match define.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (define, None),
    }
}

/// The directory `SLANG_SYS_PREBUILT_DIR` names. It must be absolute: a
/// build script runs in the crate's directory, not the caller's.
pub fn prebuilt_dir(value: &std::ffi::OsStr) -> Result<PathBuf, String> {
    let dir = PathBuf::from(value);
    if dir.is_absolute() {
        Ok(dir)
    } else {
        Err(format!(
            "SLANG_SYS_PREBUILT_DIR must be an absolute path; found `{}`",
            dir.display()
        ))
    }
}

/// The name of the manifest file in a prebuilt directory and in `OUT_DIR`.
pub const MANIFEST: &str = "slang-prebuilt.toml";

/// The short name of a compiler family, as the manifest records it.
pub fn compiler_name(id: CompilerId) -> &'static str {
    match id {
        CompilerId::Gcc => "gcc",
        CompilerId::Clang => "clang",
        CompilerId::AppleClang => "apple-clang",
    }
}

/// A C++ source whose preprocessed text names the C++ standard library, its
/// version, the libstdc++ dual ABI setting and the glibc version.
pub const PROBE: &str = r#"#include <version>
#include <cstdlib>
#if defined(_LIBCPP_VERSION)
slang_sys_stdlib "libc++" _LIBCPP_VERSION
#elif defined(__GLIBCXX__)
slang_sys_stdlib "libstdc++" _GLIBCXX_RELEASE
#else
slang_sys_stdlib "unknown" 0
#endif
#if defined(_GLIBCXX_USE_CXX11_ABI)
slang_sys_cxx11_abi _GLIBCXX_USE_CXX11_ABI
#else
slang_sys_cxx11_abi none
#endif
#if defined(__GLIBC__)
slang_sys_glibc __GLIBC__ . __GLIBC_MINOR__
#else
slang_sys_glibc none
#endif
"#;

/// What [`PROBE`] reports about the C++ and C libraries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Libraries {
    /// `libstdc++`, `libc++` or `unknown`.
    pub stdlib: String,
    /// `_GLIBCXX_RELEASE` (`14`) or `_LIBCPP_VERSION` (`190100`).
    pub stdlib_version: u32,
    /// `_GLIBCXX_USE_CXX11_ABI` (`1` or `0`), or `none` outside libstdc++.
    pub cxx11_abi: String,
    /// The glibc version (`2.41`), or `none` for another C library.
    pub glibc: String,
}

/// The facts [`PROBE`] reports, from its preprocessed text. GCC can break
/// a line where it expands a system header's macro and insert a line
/// marker, so the text is read as one token stream without marker lines:
/// a key's value is the tokens up to the next key.
pub fn parse_probe(preprocessed: &str) -> Result<Libraries, String> {
    let tokens: Vec<&str> = preprocessed
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace)
        .collect();
    let value = |key: &str| {
        let start = tokens
            .iter()
            .position(|t| *t == key)
            .ok_or_else(|| format!("the C++ library probe printed no `{key}`"))?;
        Ok::<_, String>(
            tokens[start + 1..]
                .iter()
                .take_while(|t| !t.starts_with("slang_sys_"))
                .copied()
                .collect::<Vec<_>>(),
        )
    };
    let stdlib = value("slang_sys_stdlib")?;
    let [name, version] = stdlib.as_slice() else {
        return Err(format!("unexpected probe output `{}`", stdlib.join(" ")));
    };
    let stdlib_version = version
        .parse()
        .map_err(|_| format!("the C++ library version `{version}` is not a number"))?;
    Ok(Libraries {
        stdlib: name.trim_matches('"').to_string(),
        stdlib_version,
        cxx11_abi: value("slang_sys_cxx11_abi")?.concat(),
        glibc: value("slang_sys_glibc")?.concat(),
    })
}

/// What a Slang build depends on to link with a shim: what a prebuilt
/// library was built with, or what the current build would link it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Abi {
    /// The vendored Slang release.
    pub slang: String,
    /// The Rust target triple.
    pub target: String,
    /// See [`compiler_name`].
    pub compiler: String,
    pub compiler_version: String,
    pub libraries: Libraries,
    pub std: String,
    /// Position-independent code.
    pub pic: bool,
    /// See [`SHIM_DEFINES`].
    pub defines: Vec<String>,
}

impl Abi {
    /// The manifest text.
    pub fn to_toml(&self) -> String {
        let mut t = toml::Table::new();
        let mut put = |k: &str, v: toml::Value| t.insert(k.to_string(), v);
        put("slang", self.slang.clone().into());
        put("target", self.target.clone().into());
        put("compiler", self.compiler.clone().into());
        put("compiler_version", self.compiler_version.clone().into());
        put("stdlib", self.libraries.stdlib.clone().into());
        put(
            "stdlib_version",
            i64::from(self.libraries.stdlib_version).into(),
        );
        put("cxx11_abi", self.libraries.cxx11_abi.clone().into());
        put("glibc", self.libraries.glibc.clone().into());
        put("std", self.std.clone().into());
        put("pic", self.pic.into());
        put("defines", self.defines.clone().into());
        format!(
            "# A Slang build of slang-sys; see doc/design/slang-in-process.md, \"Prebuilt\".\n{t}"
        )
    }

    /// Reads a manifest written by [`Abi::to_toml`].
    pub fn from_toml(text: &str) -> Result<Abi, String> {
        let t: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
        let get = |k: &str| t.get(k).ok_or_else(|| format!("no key `{k}`"));
        let string = |k: &str| {
            get(k)?
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("`{k}` is not a string"))
        };
        let stdlib_version = get("stdlib_version")?
            .as_integer()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or("`stdlib_version` is not a version number")?;
        let pic = get("pic")?.as_bool().ok_or("`pic` is not a boolean")?;
        let defines = get("defines")?
            .as_array()
            .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect())
            .ok_or("`defines` is not a list of strings")?;
        Ok(Abi {
            slang: string("slang")?,
            target: string("target")?,
            compiler: string("compiler")?,
            compiler_version: string("compiler_version")?,
            libraries: Libraries {
                stdlib: string("stdlib")?,
                stdlib_version,
                cxx11_abi: string("cxx11_abi")?,
                glibc: string("glibc")?,
            },
            std: string("std")?,
            pic,
            defines,
        })
    }
}

/// How a manifest value must relate to the current build's.
enum Check {
    Same,
    AtLeast,
}

/// `version` as numbers (`2.41` → `[2, 41]`), or `None` if it is not one.
fn numbers(version: &str) -> Option<Vec<u32>> {
    version.split('.').map(|p| p.parse().ok()).collect()
}

/// True if version `found` is at least `needed`, comparing the parts given.
fn at_least(found: &str, needed: &str) -> bool {
    matches!((numbers(found), numbers(needed)), (Some(f), Some(n)) if f >= n)
}

/// Ok if a library built as `prebuilt` describes links with the current
/// build `current`. Same: Slang release, target, compiler family, C++
/// standard library, dual ABI, C++ standard, position independence and
/// defines. At least as new in the current build: the compiler's major
/// version, the C++ standard library and glibc, which are backward
/// compatible but not forward.
pub fn check_prebuilt(prebuilt: &Abi, current: &Abi) -> Result<(), String> {
    let (p, c) = (prebuilt, current);
    let (pl, cl) = (&p.libraries, &c.libraries);
    let major_of = |v: &str| major(v).map(|m| m.to_string()).unwrap_or_default();
    let glibc_check = if pl.glibc == "none" || cl.glibc == "none" {
        Check::Same
    } else {
        Check::AtLeast
    };
    let checks = [
        ("slang", Check::Same, p.slang.clone(), c.slang.clone()),
        ("target", Check::Same, p.target.clone(), c.target.clone()),
        (
            "compiler",
            Check::Same,
            p.compiler.clone(),
            c.compiler.clone(),
        ),
        (
            "compiler_version",
            Check::AtLeast,
            major_of(&p.compiler_version),
            major_of(&c.compiler_version),
        ),
        ("stdlib", Check::Same, pl.stdlib.clone(), cl.stdlib.clone()),
        (
            "stdlib_version",
            Check::AtLeast,
            pl.stdlib_version.to_string(),
            cl.stdlib_version.to_string(),
        ),
        (
            "cxx11_abi",
            Check::Same,
            pl.cxx11_abi.clone(),
            cl.cxx11_abi.clone(),
        ),
        ("glibc", glibc_check, pl.glibc.clone(), cl.glibc.clone()),
        ("std", Check::Same, p.std.clone(), c.std.clone()),
        ("pic", Check::Same, p.pic.to_string(), c.pic.to_string()),
        (
            "defines",
            Check::Same,
            p.defines.join(" "),
            c.defines.join(" "),
        ),
    ];
    let wrong: Vec<String> = checks
        .iter()
        .filter_map(|(key, check, p, c)| match check {
            Check::Same if p != c => Some(format!("{key}: prebuilt `{p}`, this build `{c}`")),
            Check::AtLeast if !at_least(c, p) => Some(format!(
                "{key}: prebuilt needs `{p}` or newer, this build has `{c}`"
            )),
            _ => None,
        })
        .collect();
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the prebuilt Slang in SLANG_SYS_PREBUILT_DIR does not fit this build:\n  {}\n\
             Use a prebuilt made for this target and compiler (`cargo xtask slang-prebuilt`), \
             or unset SLANG_SYS_PREBUILT_DIR to build Slang from source.",
            wrong.join("\n  ")
        ))
    }
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
        assert!(e.contains("3.28") && e.contains("pip install cmake"), "{e}");
        assert!(check_cmake("cmake version 4.4.4").is_ok());
        assert!(check_cmake("cmake version 3.28.0").is_ok());
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

    #[test]
    fn compiler_version_numbers() {
        assert_eq!(version("g++ (GCC) 16.2.0\nCopyright"), "16.2.0");
        assert_eq!(version("Ubuntu clang version 18.1.3 (1ubuntu1)"), "18.1.3");
        assert_eq!(version(""), "");
    }

    #[test]
    fn defines_split_into_name_and_value() {
        assert_eq!(
            split_define("SLANG_USE_THREADS"),
            ("SLANG_USE_THREADS", None)
        );
        assert_eq!(
            split_define(ASSERT_DEFINE),
            ("SLANG_ASSERT_ENABLED", Some("1"))
        );
    }

    #[test]
    fn prebuilt_dir_must_be_absolute() {
        let dir = |s: &str| prebuilt_dir(std::ffi::OsStr::new(s));
        assert_eq!(dir("/opt/slang"), Ok(PathBuf::from("/opt/slang")));
        assert!(dir("prebuilt").unwrap_err().contains("absolute"));
        assert!(dir("").is_err());
    }

    #[test]
    fn probe_output_of_libstdcxx_and_libcxx() {
        // As `g++ -E` (GCC 16.2) prints it: each value that a system
        // header's macro expands to goes on a new line after a line marker.
        let gcc = "slang_sys_stdlib \"libstdc++\" \n# 6 \"probe.cc\" 3\n\
                   \x20                           16\n# 11 \"probe.cc\"\n\
                   slang_sys_cxx11_abi \n# 11 \"probe.cc\" 3\n                   1\n\
                   # 16 \"probe.cc\"\nslang_sys_glibc \n# 16 \"probe.cc\" 3\n\
                   \x20              2 \n# 16 \"probe.cc\"\n\
                   \x20                        . \n# 16 \"probe.cc\" 3\n\
                   \x20                          41\n";
        let expected = Libraries {
            stdlib: "libstdc++".into(),
            stdlib_version: 16,
            cxx11_abi: "1".into(),
            glibc: "2.41".into(),
        };
        assert_eq!(parse_probe(gcc), Ok(expected.clone()));
        // As Clang, or GCC with `-P`, prints it.
        let one_line = "slang_sys_stdlib \"libstdc++\" 16\nslang_sys_cxx11_abi 1\n\
                        slang_sys_glibc 2 . 41\n";
        assert_eq!(parse_probe(one_line), Ok(expected));
        let libcxx = "slang_sys_stdlib \"libc++\" 190100\nslang_sys_cxx11_abi none\n\
                      slang_sys_glibc none\n";
        let l = parse_probe(libcxx).unwrap();
        assert_eq!((l.stdlib.as_str(), l.stdlib_version), ("libc++", 190100));
        assert_eq!((l.cxx11_abi.as_str(), l.glibc.as_str()), ("none", "none"));
    }

    #[test]
    fn probe_output_must_be_complete() {
        let e = parse_probe("slang_sys_stdlib \"libstdc++\" 16\n").unwrap_err();
        assert!(e.contains("slang_sys_cxx11_abi"), "{e}");
        let e = parse_probe("slang_sys_stdlib \"libstdc++\" _GLIBCXX_RELEASE\n").unwrap_err();
        assert!(e.contains("_GLIBCXX_RELEASE"), "{e}");
    }

    fn abi() -> Abi {
        Abi {
            slang: "12.0".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            compiler: "gcc".into(),
            compiler_version: "13.3.0".into(),
            libraries: Libraries {
                stdlib: "libstdc++".into(),
                stdlib_version: 13,
                cxx11_abi: "1".into(),
                glibc: "2.39".into(),
            },
            std: CXX_STD.into(),
            pic: true,
            defines: SHIM_DEFINES.iter().map(|d| d.to_string()).collect(),
        }
    }

    #[test]
    fn manifest_round_trips() {
        let text = abi().to_toml();
        assert!(
            text.contains("target = \"x86_64-unknown-linux-gnu\""),
            "{text}"
        );
        assert_eq!(Abi::from_toml(&text), Ok(abi()));
    }

    #[test]
    fn manifest_errors_name_the_key() {
        let text = abi().to_toml().replace("pic = true", "");
        assert!(Abi::from_toml(&text).unwrap_err().contains("`pic`"));
        let text = abi()
            .to_toml()
            .replace("stdlib_version = 13", "stdlib_version = \"13\"");
        assert!(
            Abi::from_toml(&text)
                .unwrap_err()
                .contains("`stdlib_version`")
        );
        assert!(Abi::from_toml("not toml [").is_err());
    }

    #[test]
    fn same_abi_fits() {
        assert_eq!(check_prebuilt(&abi(), &abi()), Ok(()));
    }

    #[test]
    fn newer_compiler_and_libraries_fit() {
        let mut current = abi();
        current.compiler_version = "16.2.0".into();
        current.libraries.stdlib_version = 16;
        current.libraries.glibc = "2.41".into();
        assert_eq!(check_prebuilt(&abi(), &current), Ok(()));
        // Only the major version of the compiler counts.
        current.compiler_version = "13.2.0".into();
        assert_eq!(check_prebuilt(&abi(), &current), Ok(()));
    }

    #[test]
    fn older_compiler_and_libraries_do_not_fit() {
        let mut current = abi();
        current.compiler_version = "12.4.0".into();
        current.libraries.stdlib_version = 12;
        current.libraries.glibc = "2.36".into();
        let e = check_prebuilt(&abi(), &current).unwrap_err();
        assert!(e.contains("compiler_version: prebuilt needs `13` or newer, this build has `12`"));
        assert!(e.contains("stdlib_version: prebuilt needs `13`"), "{e}");
        assert!(e.contains("glibc: prebuilt needs `2.39`"), "{e}");
        assert!(e.contains("unset SLANG_SYS_PREBUILT_DIR"), "{e}");
    }

    #[test]
    fn each_same_key_is_checked() {
        type Change = fn(&mut Abi);
        let cases: [(&str, Change); 9] = [
            ("slang", |a| a.slang = "11.0".into()),
            ("target", |a| a.target = "x86_64-unknown-linux-musl".into()),
            ("compiler", |a| a.compiler = "clang".into()),
            ("stdlib", |a| a.libraries.stdlib = "libc++".into()),
            ("cxx11_abi", |a| a.libraries.cxx11_abi = "0".into()),
            ("glibc", |a| a.libraries.glibc = "none".into()),
            ("std", |a| a.std = "c++17".into()),
            ("pic", |a| a.pic = false),
            ("defines", |a| {
                a.defines.pop();
            }),
        ];
        for (key, change) in cases {
            let mut current = abi();
            change(&mut current);
            let e = check_prebuilt(&abi(), &current).unwrap_err();
            assert!(e.contains(&format!("\n  {key}: prebuilt `")), "{key}: {e}");
            assert_eq!(e.matches("\n  ").count(), 1, "{key}: {e}");
        }
    }
}
