// SPDX-License-Identifier: Apache-2.0

//! First-party file hygiene (port of the former pre-commit hooks).

use regex::Regex;
use regex::bytes::Regex as BytesRegex;
use std::path::Path;
use std::process::Command;

const SKIP_DIRS: [&str; 3] = ["third_party/", "crates/", "target/"];
const TRAILING_WS_EXCLUDES: [&str; 3] = [
    "tests/feedthroughs/pipeline.rs",
    "tests/modifications/stub.rs",
    "tests/hierarchy.rs",
];
const MAX_BYTES: u64 = 500 * 1024;

/// Checks every tracked or untracked-but-not-ignored first-party file under `root`.
pub fn check_repo(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("spawn git: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files failed".to_string());
    }
    let files: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Ok(check_files(root, &files))
}

/// Checks the given repo-relative `files`; returns one message per violation.
pub fn check_files(root: &Path, files: &[String]) -> Vec<String> {
    let lef_def = Regex::new(r"(?i)\.(lef|def)(\..*)?$").unwrap();
    let private_key = BytesRegex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap();
    let mut out = Vec::new();
    for rel in files {
        if SKIP_DIRS.iter().any(|d| rel.starts_with(d)) {
            continue;
        }
        let path = root.join(rel);
        if lef_def.is_match(rel) {
            out.push(format!("{rel}: LEF/DEF files must not be committed"));
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            if !path.exists() {
                out.push(format!("{rel}: broken symlink"));
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_BYTES {
            out.push(format!("{rel}: larger than 500 KB"));
        }
        let Ok(bytes) = std::fs::read(&path) else {
            out.push(format!("{rel}: unreadable"));
            continue;
        };
        if private_key.is_match(&bytes) {
            out.push(format!("{rel}: private key"));
        }
        if bytes.contains(&0) {
            continue; // binary
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        if !text.is_empty() && !text.ends_with('\n') {
            out.push(format!("{rel}: missing final newline"));
        }
        if text.ends_with("\n\n") {
            out.push(format!("{rel}: multiple trailing newlines"));
        }
        if text.contains("\r\n") {
            out.push(format!("{rel}: CRLF line ending"));
        }
        let ws_ok = TRAILING_WS_EXCLUDES.contains(&rel.as_str());
        let mut conflict_open = false;
        for (i, line) in text.lines().enumerate() {
            let n = i + 1;
            if !ws_ok && line.ends_with([' ', '\t']) {
                out.push(format!("{rel}:{n}: trailing whitespace"));
            }
            if line.starts_with("<<<<<<< ") {
                conflict_open = true;
            }
            if conflict_open
                && (line.starts_with("<<<<<<< ")
                    || line == "======="
                    || line.starts_with(">>>>>>> "))
            {
                out.push(format!("{rel}:{n}: merge conflict marker"));
            }
            if line.starts_with(">>>>>>> ") {
                conflict_open = false;
            }
        }
        if rel.ends_with(".toml") {
            if let Err(e) = text.parse::<toml::Table>() {
                out.push(format!("{rel}: invalid TOML: {}", e.message()));
            }
        } else if rel.ends_with(".yml") || rel.ends_with(".yaml") {
            use serde::Deserialize;
            for doc in serde_yaml::Deserializer::from_str(&text) {
                if let Err(e) = serde_yaml::Value::deserialize(doc) {
                    out.push(format!("{rel}: invalid YAML: {e}"));
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `content` to `name` in a fresh temp dir and checks it.
    fn check(name: &str, content: &[u8]) -> Vec<String> {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        check_files(d.path(), &[name.to_string()])
    }

    fn has(v: &[String], needle: &str) -> bool {
        v.iter().any(|m| m.contains(needle))
    }

    #[test]
    fn clean_file_passes() {
        assert!(check("a.rs", b"fn main() {}\n").is_empty());
    }

    #[test]
    fn trailing_whitespace() {
        assert!(has(&check("a.rs", b"x \n"), "a.rs:1: trailing whitespace"));
    }

    #[test]
    fn trailing_whitespace_excludes() {
        assert!(check("tests/hierarchy.rs", b"x \n").is_empty());
    }

    #[test]
    fn final_newline() {
        assert!(has(&check("a.rs", b"x"), "missing final newline"));
    }

    #[test]
    fn crlf() {
        assert!(has(&check("a.rs", b"x\r\ny\r\n"), "CRLF"));
    }

    #[test]
    fn merge_markers() {
        let v = check("a.rs", b"<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\n");
        assert_eq!(v.iter().filter(|m| m.contains("merge conflict")).count(), 3);
        // A Markdown setext underline is not a conflict.
        assert!(check("a.md", b"Title\n=======\n").is_empty());
    }

    #[test]
    fn multiple_trailing_newlines() {
        assert!(has(&check("a.rs", b"x\n\n"), "multiple trailing"));
    }

    #[test]
    fn real_repository_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let v = check_repo(root).unwrap();
        assert!(v.is_empty(), "{v:#?}");
    }

    #[test]
    fn private_key() {
        let k = format!("-----BEGIN {} PRIVATE KEY-----\n", "RSA");
        assert!(has(&check("k.txt", k.as_bytes()), "private key"));
        let mut bin = vec![0u8, 0xff];
        bin.extend_from_slice(k.as_bytes());
        assert!(has(&check("k.bin", &bin), "private key"));
    }

    #[test]
    fn lef_def() {
        assert!(has(&check("a/x.LEF", b"x\n"), "LEF/DEF"));
        assert!(has(&check("a/x.def.gz", b"x\n"), "LEF/DEF"));
        assert!(check("a/undefined.rs", b"x\n").is_empty());
    }

    #[test]
    fn large_file() {
        let big = vec![b'a'; 501 * 1024];
        assert!(has(&check("big.txt", &big), "500 KB"));
    }

    #[test]
    fn toml_and_yaml_parse() {
        assert!(has(&check("a.toml", b"a = = 1\n"), "invalid TOML"));
        assert!(check("a.toml", b"a = 1\n").is_empty());
        assert!(has(&check("a.yml", b"a: [1\n"), "invalid YAML"));
        assert!(check("a.yml", b"a: 1\n").is_empty());
    }

    #[test]
    fn broken_symlink() {
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("missing", d.path().join("l")).unwrap();
        let v = check_files(d.path(), &["l".to_string()]);
        assert!(has(&v, "broken symlink"));
    }

    #[test]
    fn skipped_dirs() {
        assert!(check("third_party/x.rs", b"x \n").is_empty());
        assert!(check("crates/x/a.rs", b"x \n").is_empty());
    }
}
