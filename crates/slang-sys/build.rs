// SPDX-License-Identifier: Apache-2.0

//! Builds the vendored Slang and its dependencies with CMake in `OUT_DIR`, or takes a
//! prebuilt one from `SLANG_SYS_PREBUILT_DIR`, then the C++ shim with `cxx-build`. See
//! `doc/design/slang-in-process.md`, "Build".

#[path = "build/support.rs"]
mod support;

include!("build/release.rs");

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
        "SLANG_SYS_PREBUILT_DIR",
        "SLANG_SYS_NO_CCACHE",
        "CXX",
        "CC",
        "CMAKE",
        "CMAKE_GENERATOR",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let abi = current_abi(&out);
    // A manifest left by an earlier source build must not outlive it.
    let _ = fs::remove_file(out.join(support::MANIFEST));
    match env::var_os("SLANG_SYS_PREBUILT_DIR") {
        Some(dir) => link_prebuilt(
            &support::prebuilt_dir(&dir).unwrap_or_else(|e| fail(&e)),
            &abi.unwrap_or_else(|e| fail(&format!("cannot check SLANG_SYS_PREBUILT_DIR: {e}"))),
        ),
        None => build_vendored(&manifest, &out, abi),
    }
}

/// Builds the vendored Slang with CMake, then the shim against it, and
/// records the build's [`support::Abi`] in `OUT_DIR` for
/// `cargo xtask slang-prebuilt`. Without a known ABI the build goes on with
/// a warning and records no manifest: only a prebuilt needs it.
fn build_vendored(manifest: &Path, out: &Path, abi: Result<support::Abi, String>) {
    check_build_tools();

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
        .cxxflag(format!("-D{}", support::ASSERT_DEFINE))
        .define("SLANG_INCLUDE_TOOLS", "OFF")
        .define("SLANG_INCLUDE_TESTS", "OFF")
        .define("SLANG_INCLUDE_INSTALL", "OFF")
        .define("SLANG_USE_MIMALLOC", "OFF")
        .define(
            "CMAKE_POSITION_INDEPENDENT_CODE",
            if support::PIC { "ON" } else { "OFF" },
        )
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
            .env("CCACHE_BASEDIR", out)
            .env("CCACHE_NOHASHDIR", "1");
    }
    let dst = cmake.build();
    let build = dst.join("build");

    compile_shim(&[
        src.join("slang/include"),
        src.join("slang/external"),
        build.join("source"),
    ]);
    link_svlang(&build.join("lib"));
    match abi {
        Ok(abi) => fs::write(out.join(support::MANIFEST), abi.to_toml())
            .unwrap_or_else(|e| panic!("writing {}: {e}", support::MANIFEST)),
        Err(e) => println!("cargo:warning=no {} written: {e}", support::MANIFEST),
    }
}

/// Links the Slang library in `dir` (see `cargo xtask slang-prebuilt`)
/// after checking its manifest against this build, and compiles the shim
/// against its headers. Needs neither CMake nor Python.
fn link_prebuilt(dir: &Path, abi: &support::Abi) {
    let manifest = dir.join(support::MANIFEST);
    let lib = dir.join("lib");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", lib.display());
    println!("cargo:rerun-if-changed={}", dir.join("include").display());
    let text = fs::read_to_string(&manifest).unwrap_or_else(|e| {
        fail(&format!(
            "SLANG_SYS_PREBUILT_DIR: cannot read {}: {e}",
            manifest.display()
        ))
    });
    let prebuilt = support::Abi::from_toml(&text)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", manifest.display())));
    support::check_prebuilt(&prebuilt, abi)
        .unwrap_or_else(|e| fail(&format!("{e}\n(manifest: {})", manifest.display())));
    compile_shim(&[dir.join("include")]);
    link_svlang(&lib);
}

/// Compiles the `cxx` bridge and the shim against Slang's headers.
fn compile_shim(includes: &[PathBuf]) {
    let mut build = cxx_build::bridge("src/lib.rs");
    build.file("shim/shim.cc").file("shim/design.cc");
    for dir in includes {
        build.include(dir);
    }
    for define in support::SHIM_DEFINES {
        let (name, value) = support::split_define(define);
        build.define(name, value);
    }
    build.std(support::CXX_STD).compile("slang_sys_shim");
}

fn link_svlang(lib: &Path) {
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=svlang");
}

/// What this build links Slang with: target, compiler and C++ library, or
/// why it is not known. Stops the build if the compiler cannot build C++20
/// Slang.
fn current_abi(out: &Path) -> Result<support::Abi, String> {
    let compiler = cc::Build::new().cpp(true).get_compiler();
    let output = run_version(compiler.path(), "--version");
    let Some(id) = support::identify(&output) else {
        return Err(format!(
            "unknown C++ compiler {}; Slang needs C++20 (GCC 11 or Clang 17)",
            compiler.path().display()
        ));
    };
    support::check_compiler(id, &output).unwrap_or_else(|e| fail(&e));
    let probe = out.join("abi_probe.cc");
    fs::write(&probe, support::PROBE).unwrap_or_else(|e| panic!("writing the probe: {e}"));
    let expanded = cc::Build::new()
        .cpp(true)
        .std(support::CXX_STD)
        .cargo_metadata(false)
        // Only the macro values matter; header warnings would be noise.
        .flag("-w")
        // No line markers; `parse_probe` also copes with them.
        .flag("-P")
        .file(&probe)
        .try_expand()
        .map_err(|e| format!("preprocessing the C++ library probe: {e}"))?;
    let libraries = support::parse_probe(&String::from_utf8_lossy(&expanded))?;
    Ok(support::Abi {
        slang: release!().to_string(),
        target: env::var("TARGET").unwrap(),
        compiler: support::compiler_name(id).to_string(),
        compiler_version: support::version(&output).to_string(),
        libraries,
        std: support::CXX_STD.to_string(),
        pic: support::PIC,
        defines: support::SHIM_DEFINES
            .iter()
            .map(|d| d.to_string())
            .collect(),
    })
}

/// Stops the build before configuring if CMake or Python cannot build
/// Slang.
fn check_build_tools() {
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
