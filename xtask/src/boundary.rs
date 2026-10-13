// SPDX-License-Identifier: Apache-2.0

//! The library must stay generic: no vendor-specific names in `src/` or `crates/*/src/`.
//!
//! It also enforces the `slang-sys` layering rule: only `crates/slang-rs`
//! depends on `slang-sys`, and nothing under `src/` names `slang_sys` or
//! `slang_rs::sys`.

use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static PHRASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)axis-aligned|[xy] axis|\w+'s axis").unwrap());
// `axi` must start a token ("s_axi_aw" counts, "taxing" does not).
static FORBIDDEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)floo|pulp|(^|[^a-z0-9])axi").unwrap());
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9_]+").unwrap());

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

/// Prose phrases that merely resemble the forbidden names. They only count as
/// benign when bounded by whitespace, line ends or light punctuation, never by
/// `.`/`-`/identifier characters (so `x axis.tdata` is still flagged).
fn strip_phrases(line: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    for m in PHRASE.find_iter(line) {
        let before_ok = line[..m.start()]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let mut after = line[m.end()..].chars();
        let after_ok = match after.next() {
            None => true,
            Some(c) if c.is_whitespace() || ",;:)\"".contains(c) => true,
            Some('.') => after.next().is_none_or(char::is_whitespace),
            Some(_) => false,
        };
        if before_ok && after_ok {
            out.push_str(&line[last..m.start()]);
            out.push(' ');
            last = m.end();
        }
    }
    out.push_str(&line[last..]);
    out
}

/// Whole identifier tokens that are ordinary words. Exact words only, so any
/// compound such as `floorplan_floo` is still checked.
fn benign_token(tok: &str, rest: &str) -> bool {
    const WORDS: [&str; 6] = [
        "floor",
        "floors",
        "floorplan",
        "floorplans",
        "floorplanner",
        "floorplanning",
    ];
    if WORDS.iter().any(|w| tok.eq_ignore_ascii_case(w)) {
        return true;
    }
    tok == "axis_coordinate"
        && !(rest.starts_with('-')
            || rest
                .strip_prefix('.')
                .is_some_and(|r| r.starts_with(|c: char| c.is_alphanumeric() || c == '_')))
}

fn line_violates(line: &str) -> bool {
    let line = strip_phrases(line);
    TOKEN
        .find_iter(&line)
        .any(|m| !benign_token(m.as_str(), &line[m.end()..]) && FORBIDDEN.is_match(m.as_str()))
}

/// Slang is reached through `slang-rs` only: of all manifests, only
/// `crates/slang-rs/Cargo.toml` may depend on `slang-sys`, and nothing under
/// `src/` may name `slang_rs::sys` or `slang_sys`.
fn check_slang_sys(root: &Path) -> Vec<String> {
    static DEPENDS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"^\s*(slang[-_]sys\s*[=.]|\[[^\]]*dependencies\.slang[-_]sys\]|.*package\s*=\s*"slang-sys")"#,
        )
        .unwrap()
    });
    static NAMES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"slang_rs\s*::\s*sys\b|\bslang_sys\b").unwrap());

    let mut manifests = Vec::new();
    collect_manifests(root, &mut manifests);
    let mut sources = Vec::new();
    collect(&root.join("src"), &mut sources);
    let mut out = Vec::new();
    let mut scan = |files: &mut Vec<PathBuf>, hit: &dyn Fn(&Path, &str) -> bool| {
        files.sort();
        for f in files.iter() {
            let Ok(text) = std::fs::read_to_string(f) else {
                continue;
            };
            let rel = f.strip_prefix(root).unwrap_or(f);
            for (i, line) in text.lines().enumerate() {
                if hit(rel, line) {
                    out.push(format!("{}:{}", rel.display(), i + 1));
                }
            }
        }
    };
    scan(&mut manifests, &|rel, line| {
        rel != Path::new("crates/slang-rs/Cargo.toml") && DEPENDS.is_match(line)
    });
    scan(&mut sources, &|_, line| NAMES.is_match(line));
    out
}

/// Every `Cargo.toml` below `dir`, except in build output, vendored code and
/// hidden directories (worktrees, `.git`).
fn collect_manifests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "vendor" {
                collect_manifests(&p, out);
            }
        } else if name == "Cargo.toml" {
            out.push(p);
        }
    }
}

/// Returns `path:line` for every offending line.
pub fn check_repo(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect(&root.join("src"), &mut files);
    if let Ok(rd) = std::fs::read_dir(root.join("crates")) {
        for e in rd.flatten() {
            collect(&e.path().join("src"), &mut files);
        }
    }
    files.sort();
    let mut out = check_slang_sys(root);
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let rel = f.strip_prefix(root).unwrap_or(&f);
        for (i, line) in text.lines().enumerate() {
            if line_violates(line) {
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
    fn evasions_are_caught_and_prose_is_benign() {
        for bad in [
            "FlooRouter",
            "FlooRoute",
            "use FlooRouterCfg;",
            "x axis.tdata",
            "y axis-tdata",
            "edge's axis.tdata",
            "axis_coordinate.tdata",
            "s_axi_aw",
            "AXIS",
            "fn f(axis: Port) {}",
            "relax axis_tvalid",
            "ready axis_tdata",
            "floorplan_floo",
            "floorplanFloo",
            "floorplan_pulp",
            "floorplan_s_axi_aw",
            "floorplan_FlooRouter",
        ] {
            assert!(line_violates(bad), "should flag: {bad}");
        }
        for ok in [
            "floorplan",
            "floor",
            "Floorplan",
            "Floorplans",
            "FLOORPLAN",
            "Floorplanner",
            "// Floorplanning-related types",
            "axis-aligned",
            "// along the X axis",
            "the edge's axis",
            "the edge's axis.",
            "let axis_coordinate = 1;",
            "f(axis_coordinate);",
        ] {
            assert!(!line_violates(ok), "should allow: {ok}");
        }
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

    #[test]
    fn only_slang_rs_depends_on_slang_sys() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "crates/slang-rs/Cargo.toml",
            "[dependencies]\nslang-sys = { path = \"../slang-sys\" }\n",
        );
        write(
            d.path(),
            "crates/slang-sys/Cargo.toml",
            "[package]\nname = \"slang-sys\"\n",
        );
        assert!(check_slang_sys(d.path()).is_empty());
        write(
            d.path(),
            "Cargo.toml",
            "[dev-dependencies]\nslang-sys = { path = \"crates/slang-sys\" }\n",
        );
        write(
            d.path(),
            "designs/x/Cargo.toml",
            "[dependencies.slang-sys]\npath = \"../../crates/slang-sys\"\n",
        );
        write(
            d.path(),
            "xtask/Cargo.toml",
            "[dependencies]\nslang_sys = { package = \"slang-sys\", path = \"x\" }\n",
        );
        assert_eq!(
            check_slang_sys(d.path()),
            [
                "Cargo.toml:2",
                "designs/x/Cargo.toml:1",
                "xtask/Cargo.toml:2"
            ]
        );
    }

    #[test]
    fn src_may_not_name_the_sys_layer() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "src/ok.rs", "use slang_rs::str2tmpfile;\n");
        write(d.path(), "tests/t.rs", "use slang_rs::sys::run;\n");
        assert!(check_slang_sys(d.path()).is_empty());
        write(d.path(), "src/a.rs", "fn f() {}\nuse slang_rs::sys::run;\n");
        write(d.path(), "src/b/c.rs", "use slang_sys::Request;\n");
        assert_eq!(check_slang_sys(d.path()), ["src/a.rs:2", "src/b/c.rs:1"]);
    }
}
