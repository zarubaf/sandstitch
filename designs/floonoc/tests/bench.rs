// SPDX-License-Identifier: Apache-2.0

//! Import cost of the small and large reference NoCs under two workloads:
//! repeated (all routers share one configuration and all cores one seed)
//! and unique (every router has distinct FIFO depths and every core a
//! distinct seed). One `#[ignore]` test per configuration and workload;
//! run each in its own process so the peak memory and the specialization
//! cache are its own:
//!
//! `cargo test --release -p floonoc --test bench -- --ignored --exact small_repeated --nocapture`

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use floonoc::*;
use topstitch::{EmitOptions, ModDef, slang_invocations};

/// Peak resident set size of this process in KiB (`VmHWM`).
fn peak_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .expect("VmHWM in /proc/self/status");
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// `spec` with every core's seed replaced by `seed`.
fn with_one_seed(spec: &MeshSpec, seed: u32) -> MeshSpec {
    let mut spec = spec.clone();
    for (x, y) in spec.routers().collect::<Vec<_>>() {
        if let Endpoint::Compute { targets } = spec.endpoint(x, y).clone() {
            let targets = targets.into_iter().map(|t| Target { seed, ..t }).collect();
            spec.set(x, y, Endpoint::Compute { targets });
        }
    }
    spec
}

/// One build of the mesh: the elapsed time and the Slang runs it took.
fn build(cfg: &FlooConfig, spec: &MeshSpec, unique: bool, dir: &Path) -> (ModDef, Duration, u64) {
    let slang = slang_invocations();
    let start = Instant::now();
    let top = mesh_with_fifo_depths(cfg, spec, dir, |x, y| {
        if unique {
            (2 + x as u32, 2 + y as u32)
        } else {
            (cfg.in_fifo_depth, cfg.out_fifo_depth)
        }
    });
    (top, start.elapsed(), slang_invocations() - slang)
}

/// The number of distinct specializations among the manifest entries whose
/// last instance name satisfies `kind`.
fn distinct(specializations: &[(String, String)], kind: impl Fn(&str) -> bool) -> usize {
    specializations
        .iter()
        .filter(|(path, _)| kind(path.rsplit('.').next().unwrap()))
        .map(|(_, hash)| hash.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

static STARTED: AtomicBool = AtomicBool::new(false);

fn run(name: &str, (cfg, spec): (FlooConfig, MeshSpec), unique: bool) {
    assert!(
        !STARTED.swap(true, Ordering::SeqCst),
        "run each bench test in its own process with --exact <name>"
    );
    let spec = if unique {
        let seeds: Vec<u32> = spec.cores().iter().map(|&(_, _, t)| t.seed).collect();
        assert_eq!(
            seeds.iter().collect::<BTreeSet<_>>().len(),
            seeds.len(),
            "{name}: the cores need distinct seeds"
        );
        spec
    } else {
        with_one_seed(&spec, 1)
    };
    let dir = tempfile::tempdir().unwrap();
    // The same directory for both builds: the configuration packages, and
    // so the specialization keys, are equal.
    let (top, cold, cold_slang) = build(&cfg, &spec, unique, dir.path());
    let (_, warm, warm_slang) = build(&cfg, &spec, unique, dir.path());

    let start = Instant::now();
    let diagnostics = top.check();
    let check = start.elapsed();
    assert!(!diagnostics.has_errors(), "{}", diagnostics.render());
    let start = Instant::now();
    let emitted = top.emit(EmitOptions::default());
    let emit = start.elapsed();
    assert!(!emitted.is_empty());

    let specializations = top.manifest().specializations;
    let routers = distinct(&specializations, |n| n.starts_with("i_router_"));
    let chimneys = distinct(&specializations, |n| n == "i_chimney");
    let cores = distinct(&specializations, |n| n.starts_with("i_core"));
    println!(
        "BENCH {name}: cold {cold:.2?} ({cold_slang} Slang runs), warm {warm:.2?} \
         ({warm_slang} Slang runs), check {check:.2?}, emit {emit:.2?}, \
         peak RSS {} MiB, specializations: {routers} router, {chimneys} chimney, {cores} core",
        peak_kib() / 1024
    );
}

#[test]
#[ignore]
fn small_repeated() {
    run("small_repeated", small(), false);
}

#[test]
#[ignore]
fn small_unique() {
    run("small_unique", small(), true);
}

#[test]
#[ignore]
fn large_repeated() {
    run("large_repeated", large(), false);
}

#[test]
#[ignore]
fn large_unique() {
    run("large_unique", large(), true);
}
