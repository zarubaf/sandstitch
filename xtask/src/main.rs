// SPDX-License-Identifier: Apache-2.0

//! `cargo xtask`: the single entry point for repository checks.

mod boundary;
mod hygiene;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repository root")
        .to_path_buf()
}

fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    eprintln!("+ cargo {}", args.join(" "));
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.args(args).current_dir(root);
    let status = cmd.status().map_err(|e| format!("spawn cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed ({status})", args.join(" ")))
    }
}

fn report(name: &str, violations: Vec<String>) -> Result<(), String> {
    if violations.is_empty() {
        eprintln!("{name}: ok");
        return Ok(());
    }
    for v in &violations {
        eprintln!("{v}");
    }
    Err(format!("{name}: {} violation(s)", violations.len()))
}

fn ci(root: &Path) -> Result<(), String> {
    run(root, &["fmt", "--all", "--", "--check"])?;
    run(
        root,
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    report("hygiene", hygiene::check_repo(root)?)?;
    report("boundary", boundary::check_repo(root))?;
    run(root, &["test", "--workspace", "--locked"])
}

fn explain(root: &Path, code: &str) -> Result<(), String> {
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("invalid code `{code}`"));
    }
    let path = root.join("doc/errors").join(format!("{code}.md"));
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("no documentation for `{code}` ({})", path.display()))?;
    print!("{text}");
    Ok(())
}

fn main() -> ExitCode {
    let root = repo_root();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["ci"] => ci(&root),
        ["explain", code] => explain(&root, code),
        _ => Err("usage: cargo xtask <ci | explain <CODE>>".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
