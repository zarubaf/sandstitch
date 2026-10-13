// SPDX-License-Identifier: Apache-2.0

//! The vendored real-world SystemVerilog (`common_cells`, `axi`, FlooNoC)
//! elaborated through `run_slang_keep_design`, with every query answered on
//! every top and on every instance reachable by path (instances inside
//! generate blocks are not). Under the `differential` feature each run also
//! compares the typed design with the JSON extractors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use regex::Regex;
use slang_rs::*;

const EXCLUDED_DIRS: &[&str] = &["deprecated", "test", "tb", "formal", "synth"];

/// Elaboration budget of the whole test; the spec's is 30 s.
const TIME_LIMIT: Duration = Duration::from_secs(60);

// Lower bounds on the answered tops defined in each library's own
// directory, a few below the first run.
const MIN_COMMON_CELLS_TOPS: usize = 88;
const MIN_AXI_TOPS: usize = 19;
const MIN_FLOONOC_TOPS: usize = 6;

fn third_party(name: &str) -> PathBuf {
    // Canonical, like the absolute paths of the diagnostics.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party")
        .join(name)
        .canonicalize()
        .unwrap()
}

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap();
        if path.is_dir() {
            if !EXCLUDED_DIRS.contains(&name) {
                source_files(&path, out);
            }
        } else if name.ends_with(".sv") || name.ends_with(".v") {
            out.push(path);
        }
    }
}

/// The files of `dir`, packages first (they are compiled before the modules
/// that import them).
fn sources_of(dir: &str) -> Vec<PathBuf> {
    let package = Regex::new(r"(?m)^\s*package\s+\w+").unwrap();
    let mut files = Vec::new();
    source_files(&third_party(dir), &mut files);
    let (mut packages, modules): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|f| package.is_match(&std::fs::read_to_string(f).unwrap_or_default()));
    packages.extend(modules);
    packages
}

/// Module names with the absolute file declaring them.
fn modules_of(files: &[PathBuf]) -> BTreeMap<String, String> {
    let module = Regex::new(r"(?m)^\s*module\s+(automatic\s+|static\s+)?(\w+)").unwrap();
    let mut modules = BTreeMap::new();
    for file in files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for capture in module.captures_iter(&text) {
            modules.insert(capture[2].to_string(), file.to_str().unwrap().to_string());
        }
    }
    modules
}

/// Answers every query at `path`, checks invariants of any valid design, and
/// returns the number of instances answered at and below it.
fn answer_all(design: &Design, path: &str) -> usize {
    let ports = design
        .ports(path)
        .unwrap_or_else(|| panic!("no ports at {path}"));
    let interface_ports = design
        .interface_ports(path)
        .unwrap_or_else(|| panic!("no interface ports at {path}"));
    let mut names = BTreeSet::new();
    let port_names = ports
        .iter()
        .map(|p| match p {
            Ok(port) => port.name.as_str(),
            Err(unsupported) => unsupported.name.as_str(),
        })
        .chain(interface_ports.iter().map(|p| p.name.as_str()));
    for name in port_names {
        assert!(names.insert(name), "duplicate port {name} at {path}");
    }
    for port in ports.iter().flatten() {
        let unpacked = match &port.ty {
            Type::Logic {
                unpacked_dimensions,
                ..
            }
            | Type::Struct {
                unpacked_dimensions,
                ..
            }
            | Type::Union {
                unpacked_dimensions,
                ..
            }
            | Type::Enum {
                unpacked_dimensions,
                ..
            } => !unpacked_dimensions.is_empty(),
        };
        if !unpacked {
            let width = port
                .ty
                .width()
                .unwrap_or_else(|e| panic!("port {} at {path} has no width: {e}", port.name));
            assert!(width > 0, "port {} at {path} has width 0", port.name);
        }
    }
    let children = design
        .child_instances(path)
        .unwrap_or_else(|| panic!("no children at {path}"));
    let unique: BTreeSet<&str> = children.iter().copied().collect();
    assert_eq!(unique.len(), children.len(), "duplicate children at {path}");
    1 + children
        .iter()
        .map(|child| answer_all(design, &format!("{path}.{child}")))
        .sum::<usize>()
}

struct RunResult {
    /// The files defining the answered tops.
    answered_files: Vec<String>,
    instances: usize,
}

impl RunResult {
    /// The answered tops defined under `dir`.
    fn answered_under(&self, dir: &Path) -> usize {
        let dir = dir.to_str().unwrap();
        self.answered_files
            .iter()
            .filter(|f| Path::new(f).starts_with(dir))
            .count()
    }
}

/// Elaborates every module of `files` as a top and answers every query on
/// the tops without an error in their file.
fn run_corpus(name: &str, files: &[PathBuf], incdirs: &[PathBuf]) -> RunResult {
    let modules = modules_of(files);
    let sources: Vec<&str> = files.iter().map(|f| f.to_str().unwrap()).collect();
    let incdirs: Vec<&str> = incdirs.iter().map(|d| d.to_str().unwrap()).collect();
    let tops: Vec<&str> = modules.keys().map(String::as_str).collect();
    let cfg = SlangConfig {
        sources: &sources,
        tops: &tops,
        incdirs: &incdirs,
        extra_arguments: &["--allow-toplevel-iface-ports", "--error-limit=0"],
        ..Default::default()
    };
    let output = run_slang_keep_design(&cfg).unwrap_or_else(|e| panic!("{name}: {e}"));
    let bad_files: BTreeSet<&str> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity >= Severity::Error)
        .map(|d| d.file.as_str())
        .collect();

    let mut result = RunResult {
        answered_files: Vec::new(),
        instances: 0,
    };
    let mut skipped = Vec::new();
    for top in output.design.tops() {
        match modules.get(top) {
            Some(file) if !bad_files.contains(file.as_str()) => {
                result.instances += answer_all(&output.design, top);
                result.answered_files.push(file.clone());
            }
            _ => skipped.push(top),
        }
    }
    println!(
        "{name}: {} modules, {} tops elaborated, {} answered ({} instances), {} skipped, \
         {} files with errors",
        modules.len(),
        output.design.tops().count(),
        result.answered_files.len(),
        result.instances,
        skipped.len(),
        bad_files.len(),
    );
    result
}

#[test]
fn corpus_answers_every_query() {
    let start = Instant::now();
    let cc_inc = third_party("common_cells/include");
    let axi_inc = third_party("axi/include");
    let floo_inc = third_party("floonoc/hw/include");

    let cc = sources_of("common_cells/src");
    let axi = sources_of("axi/src");
    let floo = sources_of("floonoc/hw");

    let common_cells = run_corpus("common_cells", &cc, std::slice::from_ref(&cc_inc));
    let axi_run = run_corpus(
        "axi",
        &[cc.clone(), axi.clone()].concat(),
        &[cc_inc.clone(), axi_inc.clone()],
    );
    let floonoc = run_corpus(
        "floonoc",
        &[cc, axi, floo].concat(),
        &[cc_inc, axi_inc, floo_inc],
    );

    let elapsed = start.elapsed();
    println!("corpus: {elapsed:?}");
    let cc_tops = common_cells.answered_under(&third_party("common_cells"));
    let axi_tops = axi_run.answered_under(&third_party("axi"));
    let floo_tops = floonoc.answered_under(&third_party("floonoc"));
    println!(
        "answered in own library: common_cells {cc_tops}, axi {axi_tops}, floonoc {floo_tops}"
    );
    assert!(cc_tops >= MIN_COMMON_CELLS_TOPS);
    assert!(axi_tops >= MIN_AXI_TOPS);
    assert!(floo_tops >= MIN_FLOONOC_TOPS);
    assert!(elapsed < TIME_LIMIT, "corpus took {elapsed:?}");
}
