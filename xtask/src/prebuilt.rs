// SPDX-License-Identifier: Apache-2.0

//! `cargo xtask slang-prebuilt <DIR>`: builds `slang-sys` from its vendored
//! Slang and copies the library, its headers, the licenses and the manifest
//! into `<DIR>/<name>`, for `SLANG_SYS_PREBUILT_DIR`. See
//! `doc/design/slang-in-process.md`, "Prebuilt".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The manifest `crates/slang-sys/build.rs` writes and reads.
const MANIFEST: &str = "slang-prebuilt.toml";

/// Header trees of a Slang build directory (`OUT_DIR` of `slang-sys`): the
/// sources' headers and the generated ones. The shim needs all of them.
const HEADER_TREES: [&str; 3] = ["src/slang/include", "src/slang/external", "build/source"];

/// License files of the projects compiled into `libsvlang.a`, from `OUT_DIR`.
const LICENSES: [(&str, &str); 3] = [
    ("src/slang/LICENSE", "slang-LICENSE"),
    ("src/fmt/LICENSE", "fmt-LICENSE"),
    ("src/tomlplusplus/LICENSE", "tomlplusplus-LICENSE"),
];

/// Builds the prebuilt directory under `dest_root` and returns its path.
pub fn run(root: &Path, dest_root: &Path) -> Result<PathBuf, String> {
    let out = build_out_dir(root)?;
    let manifest = fs::read_to_string(out.join(MANIFEST))
        .map_err(|e| format!("reading {}: {e}", out.join(MANIFEST).display()))?;
    let dest = dest_root.join(dir_name(&manifest)?);
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("removing {}: {e}", dest.display()))?;
    }
    copy_matching(&out.join("build/lib"), &dest.join("lib"), &["a"])?;
    for tree in HEADER_TREES {
        copy_matching(&out.join(tree), &dest.join("include"), &["h", "hpp"])?;
    }
    let licenses = dest.join("licenses");
    copy_matching(&out.join("src/slang/LICENSES"), &licenses, &["txt"])?;
    for (from, to) in LICENSES {
        copy_file(&out.join(from), &licenses.join(to))?;
    }
    fs::write(dest.join(MANIFEST), manifest).map_err(|e| format!("writing manifest: {e}"))?;
    Ok(dest)
}

/// Builds `slang-sys` from source and returns its `OUT_DIR`.
fn build_out_dir(root: &Path) -> Result<PathBuf, String> {
    let args = [
        "build",
        "--package",
        "slang-sys",
        "--locked",
        "--message-format=json-render-diagnostics",
    ];
    eprintln!("+ cargo {}", args.join(" "));
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(args)
        .current_dir(root)
        // A prebuilt Slang must come from source, not from another prebuilt.
        .env_remove("SLANG_SYS_PREBUILT_DIR")
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("spawn cargo: {e}"))?;
    if !output.status.success() {
        return Err(format!("building slang-sys failed ({})", output.status));
    }
    out_dir(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "cargo reported no build script run of slang-sys".to_string())
}

/// The `OUT_DIR` of `slang-sys` in Cargo's JSON messages.
fn out_dir(messages: &str) -> Option<PathBuf> {
    messages.lines().find_map(|line| {
        let m: serde_json::Value = serde_json::from_str(line).ok()?;
        let ours =
            m["reason"] == "build-script-executed" && is_slang_sys(m["package_id"].as_str()?);
        ours.then(|| m["out_dir"].as_str().map(PathBuf::from))?
    })
}

/// True for the package id of `slang-sys`, in Cargo's current format
/// (`path+file:///…/crates/slang-sys#0.1.0`) or the old one
/// (`slang-sys 0.1.0 (path+file:///…)`). A checkout path may itself
/// contain `slang-sys`.
fn is_slang_sys(id: &str) -> bool {
    id.starts_with("slang-sys ")
        || id.rsplit_once('#').is_some_and(|(path, version)| {
            version.starts_with("slang-sys@") || path.ends_with("/slang-sys")
        })
}

/// The directory name of a prebuilt: Slang release, target and compiler
/// with its major version, `slang-12.0-x86_64-unknown-linux-gnu-gcc13`.
fn dir_name(manifest: &str) -> Result<String, String> {
    let t: toml::Table = manifest.parse().map_err(|e| format!("manifest: {e}"))?;
    let get = |k: &str| {
        t.get(k)
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("manifest: no string `{k}`"))
    };
    let major = get("compiler_version")?.split('.').next().unwrap_or("");
    Ok(format!(
        "slang-{}-{}-{}{major}",
        get("slang")?,
        get("target")?,
        get("compiler")?
    ))
}

/// Copies the files under `src` whose extension is one of `extensions` to
/// the same relative paths under `dst`. Fails if a file is already there,
/// so trees copied into one directory cannot shadow each other.
fn copy_matching(src: &Path, dst: &Path, extensions: &[&str]) -> Result<(), String> {
    let entries = fs::read_dir(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| format!("{e}"))?.path();
        let to = dst.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_matching(&path, &to, extensions)?;
        } else if path
            .extension()
            .is_some_and(|x| extensions.iter().any(|e| x == *e))
        {
            if to.exists() {
                return Err(format!("{} is already copied", to.display()));
            }
            copy_file(&path, &to)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to.parent().unwrap()).map_err(|e| format!("{e}"))?;
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("copying {}: {e}", from.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_dir_of_slang_sys() {
        let messages = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/crates/slang-sys#0.1.0"}
{"reason":"build-script-executed","package_id":"registry+https://github.com/rust-lang/crates.io-index#cxx@1.0.0","out_dir":"/t/cxx/out"}
{"reason":"build-script-executed","package_id":"path+file:///w/crates/slang-sys#0.1.0","out_dir":"/t/slang-sys-1/out"}
not json
"#;
        assert_eq!(out_dir(messages), Some(PathBuf::from("/t/slang-sys-1/out")));
        assert_eq!(out_dir("{\"reason\":\"build-finished\"}\n"), None);
    }

    #[test]
    fn package_id_of_slang_sys() {
        assert!(is_slang_sys("path+file:///w/crates/slang-sys#0.1.0"));
        assert!(is_slang_sys("path+file:///w/slang#slang-sys@0.1.0"));
        assert!(is_slang_sys(
            "slang-sys 0.1.0 (path+file:///w/crates/slang-sys)"
        ));
        // A checkout named after the crate holds other packages too.
        assert!(!is_slang_sys(
            "path+file:///w/feat-slang-sys-x/crates/dep#0.1.0"
        ));
        assert!(!is_slang_sys(
            "path+file:///w/slang-sys/crates/slang-rs#0.1.0"
        ));
    }

    #[test]
    fn dir_name_has_release_target_and_compiler() {
        let manifest = "slang = \"12.0\"\ntarget = \"x86_64-unknown-linux-gnu\"\n\
                        compiler = \"gcc\"\ncompiler_version = \"13.3.0\"\n";
        assert_eq!(
            dir_name(manifest).unwrap(),
            "slang-12.0-x86_64-unknown-linux-gnu-gcc13"
        );
        assert!(
            dir_name("compiler_version = \"13.3.0\"\n")
                .unwrap_err()
                .contains("no string `")
        );
    }

    #[test]
    fn copies_matching_files_and_rejects_shadowing() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, dst) = (
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("dst"),
        );
        fs::create_dir_all(a.join("x")).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("x/one.h"), "1").unwrap();
        fs::write(a.join("x/skip.cpp"), "").unwrap();
        fs::write(b.join("two.hpp"), "2").unwrap();
        copy_matching(&a, &dst, &["h", "hpp"]).unwrap();
        copy_matching(&b, &dst, &["h", "hpp"]).unwrap();
        assert_eq!(fs::read_to_string(dst.join("x/one.h")).unwrap(), "1");
        assert!(dst.join("two.hpp").exists());
        assert!(!dst.join("x/skip.cpp").exists());
        let e = copy_matching(&a, &dst, &["h"]).unwrap_err();
        assert!(e.contains("already copied"), "{e}");
    }
}
