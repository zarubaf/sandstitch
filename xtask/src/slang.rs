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

/// Returns the path of the `slang` binary from the verified pinned tarball,
/// downloading and extracting it if the cache is incomplete.
pub fn ensure() -> Result<PathBuf, String> {
    let dir = cache_dir()?;
    let bin = dir.join("slang");
    let tarball = dir.join("slang.tar.gz");
    if bin.is_file() && tarball_ok(&tarball)? {
        return Ok(bin);
    }
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
            return Err(format!("curl failed ({status})"));
        }
        let got = sha256_hex(&tarball)?;
        if got != SHA256 {
            let _ = std::fs::remove_file(&tarball);
            return Err(format!("sha256 mismatch: expected {SHA256}, got {got}"));
        }
    }
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir)
        .arg("slang")
        .status()
        .map_err(|e| format!("spawn tar: {e}"))?;
    if !status.success() {
        return Err(format!("tar failed ({status})"));
    }
    Ok(bin)
}
