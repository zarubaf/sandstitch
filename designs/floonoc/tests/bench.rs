// SPDX-License-Identifier: Apache-2.0

//! Import cost of the big 2x2 mesh: wall time and peak memory of this
//! process and of any child processes it ran. Run on demand:
//! `cargo test --release -p floonoc --test bench -- --ignored --nocapture`.

use std::time::Instant;

use floonoc::{FlooConfig, mesh_2x2};

/// Peak resident set size of this process in KiB (`VmHWM`).
fn own_peak_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .expect("VmHWM in /proc/self/status");
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// Largest peak resident set size of any terminated child in KiB.
fn children_peak_kib() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` fills the struct it is given.
    let usage = unsafe {
        assert_eq!(
            libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()),
            0
        );
        usage.assume_init()
    };
    usage.ru_maxrss as u64
}

#[test]
#[ignore]
fn import_cost() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let mesh = mesh_2x2(&FlooConfig::synth("big"), dir.path());
    let elapsed = start.elapsed();
    assert_eq!(mesh.get_instances().len(), 4);
    println!(
        "import_cost: elapsed {elapsed:?}, own peak {} KiB, children peak {} KiB",
        own_peak_kib(),
        children_peak_kib()
    );
}
