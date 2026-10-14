// SPDX-License-Identifier: Apache-2.0

//! Builds the vendored Slang and its dependencies with CMake in `OUT_DIR`, then the C++
//! shim with `cxx-build`. See `doc/design/slang-in-process.md`, "Build".

#[path = "build/support.rs"]
mod support;

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for path in ["vendor", "shim", "build", "src/lib.rs"] {
        println!("cargo:rerun-if-changed={path}");
    }
    for var in [
        "SLANG_SYS_NO_CCACHE",
        "CXX",
        "CC",
        "CMAKE",
        "CMAKE_GENERATOR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    check_requirements();

    let src = out.join("src");
    sync(&manifest.join("vendor"), &src).unwrap_or_else(|e| panic!("copying vendor/: {e}"));

    let launcher = support::launcher(
        which::which("ccache").ok(),
        env::var_os("SLANG_SYS_NO_CCACHE").is_some(),
    );
    let mut cmake = cmake::Config::new(src.join("slang"));
    cmake
        .no_default_flags(true)
        .profile("Release")
        .build_target("slang_slang")
        .cxxflag("-DSLANG_ASSERT_ENABLED=1")
        .define("SLANG_INCLUDE_TOOLS", "OFF")
        .define("SLANG_INCLUDE_TESTS", "OFF")
        .define("SLANG_INCLUDE_INSTALL", "OFF")
        .define("SLANG_USE_MIMALLOC", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_CXX_SCAN_FOR_MODULES", "OFF")
        // Slang puts its libraries in `build/<libdir>`; some distributions
        // (Nix, Fedora) default to `lib64`, but the link search path is `lib`.
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .define("SLANG_USE_SYSTEM_FMT", "OFF")
        .define("SLANG_USE_SYSTEM_BOOST", "OFF")
        .define("SLANG_USE_SYSTEM_TOMLPLUSPLUS", "OFF")
        .define("FETCHCONTENT_SOURCE_DIR_FMT", src.join("fmt"))
        .define(
            "FETCHCONTENT_SOURCE_DIR_TOMLPLUSPLUS",
            src.join("tomlplusplus"),
        )
        .define(
            "FETCHCONTENT_SOURCE_DIR_BOOST_REGEX",
            src.join("boost_regex"),
        )
        .define("FETCHCONTENT_FULLY_DISCONNECTED", "ON")
        .define("FETCHCONTENT_TRY_FIND_PACKAGE_MODE", "NEVER");
    // CMake rejects a generator that differs from the build directory's.
    let cache = fs::read_to_string(out.join("build/CMakeCache.txt")).ok();
    if let Some(generator) = support::generator(cache.as_deref(), which::which("ninja").is_ok()) {
        cmake.generator(generator);
    }
    // Always defined: an empty value overrides a launcher cached earlier.
    let launcher_value = support::launcher_value(launcher.as_ref());
    cmake
        .define("CMAKE_C_COMPILER_LAUNCHER", &launcher_value)
        .define("CMAKE_CXX_COMPILER_LAUNCHER", &launcher_value);
    if launcher.is_some() {
        cmake
            .env("CCACHE_BASEDIR", &out)
            .env("CCACHE_NOHASHDIR", "1");
    }
    let dst = cmake.build();
    let build = dst.join("build");

    cxx_build::bridge("src/lib.rs")
        // GCC reports false positives in the bridge code cxx generates (the
        // empty `rust::Vec` constructors) when optimizing.
        .flag_if_supported("-Wno-maybe-uninitialized")
        .file("shim/shim.cc")
        .file("shim/design.cc")
        .include(src.join("slang/include"))
        .include(src.join("slang/external"))
        .include(build.join("source"))
        .define("SLANG_STATIC_DEFINE", None)
        .define("SLANG_BOOST_SINGLE_HEADER", None)
        .define("SLANG_USE_THREADS", None)
        .define("SLANG_ASSERT_ENABLED", Some("1"))
        .std("c++20")
        .compile("slang_sys_shim");

    println!(
        "cargo:rustc-link-search=native={}",
        build.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=svlang");
}

/// Stops the build before configuring if the compiler, CMake or Python
/// cannot build Slang.
fn check_requirements() {
    let compiler = cc::Build::new().cpp(true).get_compiler();
    let output = run_version(compiler.path(), "--version");
    match support::identify(&output) {
        Some(id) => support::check_compiler(id, &output).unwrap_or_else(|e| fail(&e)),
        None => println!(
            "cargo:warning=unknown C++ compiler {}; Slang needs C++20 (GCC 11 or Clang 17)",
            compiler.path().display()
        ),
    }
    let cmake = env::var_os("CMAKE").unwrap_or_else(|| "cmake".into());
    support::check_cmake(&run_version(Path::new(&cmake), "--version")).unwrap_or_else(|e| fail(&e));
    support::check_python(which::which("python3").is_ok()).unwrap_or_else(|e| fail(&e));
}

/// The output of `program flag`, or an empty string if it cannot run.
fn run_version(program: &Path, flag: &str) -> String {
    Command::new(program)
        .arg(flag)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    panic!("{message}");
}

/// Makes `dst` a copy of `src`: copies files that are missing or differ,
/// with their modification times, and removes files `src` does not have.
/// Unchanged files keep their times, so CMake does not rebuild Slang.
fn sync(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    let mut wanted = std::collections::HashSet::new();
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        wanted.insert(entry.file_name());
        if entry.file_type()?.is_dir() {
            sync(&from, &to)?;
        } else if !same_file_content(&from, &to)? {
            fs::copy(&from, &to)?;
            let mtime = filetime::FileTime::from_last_modification_time(&entry.metadata()?);
            filetime::set_file_mtime(&to, mtime)?;
        }
    }
    for entry in fs::read_dir(dst)? {
        let entry = entry?;
        if !wanted.contains(&entry.file_name()) {
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

fn same_file_content(a: &Path, b: &Path) -> io::Result<bool> {
    let Ok(b_meta) = fs::metadata(b) else {
        return Ok(false);
    };
    if fs::metadata(a)?.len() != b_meta.len() {
        return Ok(false);
    }
    Ok(fs::read(a)? == fs::read(b)?)
}
