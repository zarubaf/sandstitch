// SPDX-License-Identifier: Apache-2.0

//! Import cost of the small and large reference NoCs under two workloads:
//! repeated (all routers share one configuration and all cores one seed)
//! and unique (every router has distinct FIFO depths and every core a
//! distinct seed). One `#[ignore]` test per configuration and workload;
//! run each in its own process so the peak memory and the specialization
//! cache are its own:
//!
//! `cargo test --release -p floonoc --test bench -- --ignored --exact small_repeated --nocapture`
//!
//! Each run also prints PASS or FAIL against the time and memory budgets.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use floonoc::*;
use topstitch::EmitOptions;

mod common;
use common::{Measured, measure, peak_kib};

/// The recorded cold build time (ms) and peak RSS (MiB) of each workload
/// (doc/design/reference-noc.md, "Measurements"). A cold build over twice
/// the time or a peak RSS over 1.5 times the value fails its budget; the
/// check is meaningful on the recorded machine only.
const RECORDED: [(&str, f64, u64); 4] = [
    ("small_repeated", 178.0, 38),
    ("small_unique", 288.0, 40),
    ("large_repeated", 212.0, 44),
    ("large_unique", 532.0, 49),
];

static STARTED: AtomicBool = AtomicBool::new(false);

fn run(name: &str, workload: (FlooConfig, MeshSpec), unique: bool) {
    assert!(
        !STARTED.swap(true, Ordering::SeqCst),
        "run each bench test in its own process with --exact <name>"
    );
    let Measured {
        top,
        cold,
        cold_slang,
        warm,
        warm_slang,
        routers,
        chimneys,
        cores,
    } = measure(name, workload, unique);

    let start = Instant::now();
    let diagnostics = top.check();
    let check = start.elapsed();
    assert!(!diagnostics.has_errors(), "{}", diagnostics.render());
    let start = Instant::now();
    let emitted = top.emit(EmitOptions::default());
    let emit = start.elapsed();
    assert!(!emitted.is_empty());

    let rss = peak_kib() / 1024;
    println!(
        "BENCH {name}: cold {cold:.2?} ({cold_slang} Slang runs), warm {warm:.2?} \
         ({warm_slang} Slang runs), check {check:.2?}, emit {emit:.2?}, \
         peak RSS {rss} MiB, specializations: {routers} router, {chimneys} chimney, {cores} core",
    );

    let &(_, cold_ms, rss_mib) = RECORDED.iter().find(|(n, ..)| *n == name).unwrap();
    let verdict = |ok: bool| if ok { "PASS" } else { "FAIL" };
    let cold_now = cold.as_secs_f64() * 1e3;
    let rss_limit = rss_mib as f64 * 1.5;
    let cold_ok = cold_now <= 2.0 * cold_ms;
    let rss_ok = rss as f64 <= rss_limit;
    println!(
        "BUDGET {name}: cold {cold_now:.0} ms <= {:.0} ms (2x recorded): {}; \
         peak RSS {rss} MiB <= {rss_limit:.0} MiB (1.5x recorded): {}",
        2.0 * cold_ms,
        verdict(cold_ok),
        verdict(rss_ok),
    );
    if cfg!(debug_assertions) {
        println!(
            "BUDGET {name}: check skipped; the budgets are meaningful only with \
             --release on the recorded machine"
        );
    } else {
        assert!(cold_ok, "{name}: cold build over budget");
        assert!(rss_ok, "{name}: peak RSS over budget");
    }
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
