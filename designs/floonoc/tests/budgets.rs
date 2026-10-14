// SPDX-License-Identifier: Apache-2.0

//! Regression budgets of the reference NoC workloads that are exact and
//! machine independent: Slang invocations and specialization counts. Time
//! and memory budgets are checked by the ignored `bench.rs`.
//!
//! One test, so the process-wide `topstitch::slang_invocations()` counter
//! and the in-process import caches are not shared with other tests. The
//! workloads run in a fixed order and the first-build counts are deltas in
//! that order. They equal the per-process bench counts: each workload
//! builds into its own directory, so no import of an earlier workload is
//! reused.

use std::sync::atomic::{AtomicBool, Ordering};

use floonoc::*;

mod common;
use common::measure;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Expected counts of one workload.
struct Budget {
    name: &'static str,
    /// Slang runs of the first build.
    cold_slang: u64,
    /// Router, chimney and core specializations.
    specializations: (usize, usize, usize),
}

#[test]
fn reference_noc_budgets() {
    assert!(
        !STARTED.swap(true, Ordering::SeqCst),
        "this binary holds one test: the Slang counter and import caches are process-wide"
    );
    let workloads: [(Budget, (FlooConfig, MeshSpec), bool); 4] = [
        (
            Budget {
                name: "small_repeated",
                cold_slang: 10,
                specializations: (1, 2, 1),
            },
            small(),
            false,
        ),
        (
            Budget {
                name: "small_unique",
                cold_slang: 15,
                specializations: (4, 2, 3),
            },
            small(),
            true,
        ),
        (
            Budget {
                name: "large_repeated",
                cold_slang: 14,
                specializations: (1, 2, 2),
            },
            large(),
            false,
        ),
        (
            Budget {
                name: "large_unique",
                cold_slang: 34,
                specializations: (9, 2, 14),
            },
            large(),
            true,
        ),
    ];
    for (budget, workload, unique) in workloads {
        let m = measure(budget.name, workload, unique);
        assert_eq!(
            (m.cold_slang, m.warm_slang, (m.routers, m.chimneys, m.cores)),
            (budget.cold_slang, 0, budget.specializations),
            "{}",
            budget.name
        );
    }
}
