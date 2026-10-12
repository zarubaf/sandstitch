// SPDX-License-Identifier: Apache-2.0

//! The library must stay generic: no vendor-specific names in `src/` or `crates/*/src/`.

use regex::Regex;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Returns `path:line` for every offending line.
pub fn check_repo(root: &Path) -> Vec<String> {
    // `axi` must start a token ("s_axi_aw" counts, "taxing" does not).
    let re = Regex::new(r"(?i)floo|pulp|(^|[^a-z0-9])axi").unwrap();
    // Ordinary phrases that merely resemble the forbidden names.
    let benign =
        Regex::new(r"(?i)floor|axis-aligned|\b[xy] axis\b|\baxis_coordinate\b|'s axis\b").unwrap();
    Regex::new(r"(?i)floor|axis-aligned|[xy] axis|(?-i:\baxis(_coordinate)?\b)").unwrap();
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        for e in rd.flatten() {
            collect(&e.path().join("src"), &mut files);
        }
    }
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let rel = f.strip_prefix(root).unwrap_or(&f);
        for (i, line) in text.lines().enumerate() {
            if re.is_match(&benign.replace_all(line, "")) {
                out.push(format!("{}:{}", rel.display(), i + 1));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn reports_offenders_with_line_numbers() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "src/a.rs", "fn ok() {}\n// FlooNoC\n");
        write(d.path(), "crates/x/src/b.rs", "let pulp = 1;\n");
        write(d.path(), "crates/x/src/c.rs", "// AXI4\n");
        write(d.path(), "tests/t.rs", "floo pulp axi\n");
        assert_eq!(
            check_repo(d.path()),
            ["crates/x/src/b.rs:1", "crates/x/src/c.rs:1", "src/a.rs:2"]
        );
    }

    #[test]
    fn clean_tree_passes() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "src/a.rs",
            "fn taxing() {} // axis-aligned floorplan, X axis, the edge's axis\nlet axis_coordinate = 1;\n",
        );
        assert!(check_repo(d.path()).is_empty());
    }

    #[test]
    fn axi_stream_slips_are_caught() {
        let d = tempfile::tempdir().unwrap();
        let cases = [
            "fn f(axis: Port) {}",
            "axis.tdata",
            "port(\"axis\")",
            "ready axis_tdata",
            "relax axis_tvalid",
        ];
        for (i, l) in cases.iter().enumerate() {
            write(d.path(), &format!("src/f{i}.rs"), &format!("{l}\n"));
        }
        assert_eq!(check_repo(d.path()).len(), cases.len());
    }

    #[test]
    fn token_forms_are_caught() {
        let d = tempfile::tempdir().unwrap();
        for (i, l) in ["s_axi_aw", "m_axi_b", "AXIS", "floo_", "floonoc"]
            .iter()
            .enumerate()
        {
            write(d.path(), &format!("src/f{i}.rs"), &format!("{l}\n"));
        }
        assert_eq!(check_repo(d.path()).len(), 5);
    }
}
