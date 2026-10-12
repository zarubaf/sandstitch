// SPDX-License-Identifier: Apache-2.0

//! Pinned Slang release: download, verify (sha256 via the `sha2` crate), cache.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "v11.0";
const SHA256: &str = "951a170e10e25e54c91565030acfdfc11c3226714ebf225a18ad4166a898d8a4";
const URL: &str =
    "https://github.com/MikePopoloski/slang/releases/download/v11.0/slang-linux-x86_64.tar.gz";

fn sha256_hex(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn cache_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(format!(".cache/sandstitch/slang-{VERSION}")))
}

fn tarball_ok(tarball: &Path) -> Result<bool, String> {
    Ok(tarball.is_file() && sha256_hex(tarball)? == SHA256)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Returns the path of the `slang` binary from the verified pinned tarball,
/// downloading and extracting it if the cache is incomplete.
pub fn ensure() -> Result<PathBuf, String> {
    if (std::env::consts::OS, std::env::consts::ARCH) != ("linux", "x86_64") {
        return Err(format!(
            "the pinned Slang release is linux-x86_64 only; this is {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    let dir = cache_dir()?;
    let bin = dir.join("slang");
    let tarball = dir.join("slang.tar.gz");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    if !tarball_ok(&tarball)? {
        eprintln!("downloading Slang {VERSION}");
        let status = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--proto",
                "=https",
                "--tlsv1.2",
                "-o",
            ])
            .arg(&tarball)
            .arg(URL)
            .status()
            .map_err(|e| format!("spawn curl: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&tarball);
            return Err(format!("curl failed ({status})"));
        }
        let got = sha256_hex(&tarball)?;
        if got != SHA256 {
            let _ = std::fs::remove_file(&tarball);
            return Err(format!("sha256 mismatch: expected {SHA256}, got {got}"));
        }
    }
    // The marker records the tarball digest the binary was extracted from, so
    // a truncated or foreign binary is never trusted.
    let marker = dir.join("slang.ok");
    let marker_ok = std::fs::read_to_string(&marker).is_ok_and(|m| m.trim() == SHA256);
    if marker_ok && is_executable(&bin) {
        return Ok(bin);
    }
    let _ = std::fs::remove_file(&marker);
    // Extract into a private temp dir, then rename into place atomically.
    let tmp = dir.join(format!("extract.{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("mkdir {}: {e}", tmp.display()))?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&tmp)
        .arg("slang")
        .status()
        .map_err(|e| format!("spawn tar: {e}"))?;
    let extracted = tmp.join("slang");
    let result = if !status.success() {
        Err(format!("tar failed ({status})"))
    } else if !is_executable(&extracted) {
        Err("extracted slang is not an executable file".to_string())
    } else {
        std::fs::rename(&extracted, &bin).map_err(|e| format!("install slang: {e}"))
    };
    let _ = std::fs::remove_dir_all(&tmp);
    result?;
    std::fs::write(&marker, format!("{SHA256}\n")).map_err(|e| format!("write marker: {e}"))?;
    Ok(bin)
}
