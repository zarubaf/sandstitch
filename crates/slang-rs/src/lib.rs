// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use serde_json::Value;
use std::fs::{self, write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

mod extract;
pub use extract::{
    Field, InterfacePort, ParameterDef, Port, PortDir, Range, SourceLoc, Type, Variant,
    extract_instance_interface_ports_from_value, extract_instance_ports_from_value,
    extract_interface_ports_from_value, extract_modules, extract_modules_from_value,
    extract_parameter_defs, extract_parameter_defs_from_value, extract_ports,
    extract_ports_from_value, extract_type_parameter_names_from_value, parse_type_definition,
};

mod hierarchy;
pub use hierarchy::{Instance, extract_hierarchy, extract_hierarchy_from_value};

mod package;
pub use package::{Package, Parameter, extract_packages, extract_packages_from_value};

#[derive(Debug)]
pub struct SlangConfig<'a> {
    pub sources: &'a [&'a str],
    pub tops: &'a [&'a str],
    pub incdirs: &'a [&'a str],
    pub defines: &'a [(&'a str, &'a str)],
    pub parameters: &'a [(&'a str, &'a str)],
    pub libfiles: &'a [&'a str],
    pub libdirs: &'a [&'a str],
    pub libexts: &'a [&'a str],
    pub ignore_unknown_modules: bool,
    pub ignore_protected: bool,
    pub capture_stdio: bool,
    pub timescale: Option<&'a str>,
    pub extra_arguments: &'a [&'a str],
}

impl<'a> Default for SlangConfig<'a> {
    fn default() -> Self {
        SlangConfig {
            sources: &[],
            tops: &[],
            incdirs: &[],
            defines: &[],
            parameters: &[],
            libfiles: &[],
            libdirs: &[],
            libexts: &[],
            ignore_unknown_modules: true,
            ignore_protected: true,
            capture_stdio: true,
            timescale: None,
            extra_arguments: &[],
        }
    }
}

/// Searches the PATH to try and locate the slang binary.
fn find_slang() -> Option<String> {
    which::which("slang")
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

/// Adds options needed to make slang ignore protected envelopes.
fn push_options_to_ignore_protected(args: &mut Vec<&str>) {
    let options = vec![
        "--enable-legacy-protect",
        "-Wno-protected-envelope",
        "-Wno-expected-protect-arg",
        "-Wno-expected-protect-keyword",
        "-Wno-extra-protect-end",
        "-Wno-invalid-encoding-byte",
        "-Wno-invalid-pragma-number",
        "-Wno-invalid-pragma-viewport",
        "-Wno-nested-protect-begin",
        "-Wno-protect-arglist",
        "-Wno-protect-encoding-bytes",
        "-Wno-protected-envelope",
        "-Wno-raw-protect-eof",
        "-Wno-unknown-protect-encoding",
        "-Wno-unknown-protect-keyword",
        "-Wno-unknown-protect-option",
    ];

    for option in options {
        args.push(option);
    }
}

pub fn run_slang(cfg: &SlangConfig) -> Result<Value, Box<dyn std::error::Error>> {
    // Run the slang binary, dumping JSON to tmp_json.

    let slang_path = std::env::var("SLANG_PATH").unwrap_or_else(|_| {
        // fallback solution: try to find slang on our own
        match find_slang() {
            Some(val) => val,
            None => panic!(
                "Please set the SLANG_PATH environment variable to the path of the slang binary."
            ),
        }
    });

    if !std::path::Path::new(&slang_path).exists() {
        return Err(format!("slang binary not found at path: {}", slang_path).into());
    }

    let tmp_json = tempfile::NamedTempFile::new()?;
    // Source info adds `source_file`/`source_line`/`source_column` to each
    // symbol, which is used to fill in declaration locations (e.g. `Port::loc`).
    let mut args = vec![
        "--ast-json",
        tmp_json.path().to_str().unwrap(),
        "--ast-json-source-info",
    ];

    if cfg.ignore_unknown_modules {
        args.push("--ignore-unknown-modules");
    }

    if cfg.ignore_protected {
        push_options_to_ignore_protected(&mut args);
    }

    for extra_arg in cfg.extra_arguments.iter() {
        args.push(extra_arg);
    }

    let param_args: Vec<String> = cfg
        .parameters
        .iter()
        .map(|(name, value)| format!("{}={}", name, value))
        .collect();
    for param_arg in param_args.iter() {
        args.push("-G");
        args.push(param_arg.as_str());
    }

    for top in cfg.tops.iter() {
        args.push("--top");
        args.push(top);
    }

    for incdir in cfg.incdirs.iter() {
        args.push("-I");
        args.push(incdir);
    }

    let define_args: Vec<String> = cfg
        .defines
        .iter()
        .map(|(name, value)| format!("{}={}", name, value))
        .collect();
    for define_arg in define_args.iter() {
        args.push("-D");
        args.push(define_arg.as_str());
    }

    for libfile in cfg.libfiles.iter() {
        args.push("-v");
        args.push(libfile);
    }

    for libdir in cfg.libdirs.iter() {
        args.push("-y");
        args.push(libdir);
    }

    for libext in cfg.libexts.iter() {
        args.push("-Y");
        args.push(libext);
    }

    if let Some(timescale) = cfg.timescale {
        args.push("--timescale");
        args.push(timescale);
    }

    for source in cfg.sources.iter() {
        args.push(source);
    }

    // Slang reports source paths relative to its working directory, which is
    // this process's; capture it so they can be made absolute.
    let working_dir = std::env::current_dir()?;
    let mut cmd = Command::new(slang_path);

    cmd.args(args);
    if !cfg.capture_stdio {
        cmd.stderr(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
    }

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = if cfg.capture_stdio {
            String::from_utf8_lossy(&output.stderr).to_string()
        } else {
            "Not Captured".to_string()
        };
        return Err(format!(
            "slang command failed with exit code: {}, stderr: {stderr}",
            output.status,
        )
        .into());
    }

    // Read and parse the JSON output
    let json_data = fs::read_to_string(tmp_json)?;
    let mut json_deserializer = serde_json::Deserializer::from_str(&json_data);
    // The recursion limit needs to be disabled since slang can produce
    // deeply nested objects.
    // Follows the example code here:
    // https://docs.rs/serde_json/latest/serde_json/struct.Deserializer.html#method.disable_recursion_limit
    json_deserializer.disable_recursion_limit();

    let stacked_deserializer = serde_stacker::Deserializer::new(&mut json_deserializer);
    let mut json_value = Value::deserialize(stacked_deserializer)?;
    absolutize_source_files(&mut json_value, &working_dir);

    Ok(json_value)
}

/// Rewrites the `source_file*` fields added by `--ast-json-source-info`
/// (relative to Slang's working directory) to absolute, lexically normalized
/// paths. Pseudo-files such as `<command-line>` are left unchanged.
fn absolutize_source_files(value: &mut Value, working_dir: &Path) {
    // Iterative: the AST can be nested deeper than the native stack allows.
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                for (key, child) in object.iter_mut() {
                    if key.starts_with("source_file") {
                        if let Value::String(path) = child {
                            if !path.starts_with('<') {
                                *path = absolute_path(working_dir, path);
                            }
                        }
                    } else {
                        stack.push(child);
                    }
                }
            }
            Value::Array(array) => stack.extend(array.iter_mut()),
            _ => {}
        }
    }
}

fn absolute_path(working_dir: &Path, path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in working_dir.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized.to_string_lossy().into_owned()
}

pub fn str2tmpfile(s: &str) -> Result<tempfile::NamedTempFile, Box<dyn std::error::Error>> {
    let tmp_file = tempfile::NamedTempFile::new()?;
    write(tmp_file.path(), s)?;
    Ok(tmp_file)
}
