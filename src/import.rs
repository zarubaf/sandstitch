// SPDX-License-Identifier: Apache-2.0

//! SystemVerilog import through Slang: parser configuration, module and
//! port import, parameterization, and package extraction.

pub(crate) mod hierarchy;
pub(crate) mod package;
pub(crate) mod parameterize;
pub(crate) mod parser;
pub(crate) mod parser_cfg;
pub(crate) mod specialize;
pub(crate) mod typed;

use std::sync::atomic::{AtomicU64, Ordering};

static SLANG_INVOCATIONS: AtomicU64 = AtomicU64::new(0);

/// Counts one Slang run; called before every `slang_rs` call that runs it.
pub(crate) fn count_slang_run() {
    SLANG_INVOCATIONS.fetch_add(1, Ordering::Relaxed);
}

/// The number of Slang runs this process has started so far: imports,
/// parameter elaborations, parse checks and package extractions. Take the difference
/// between two readings to measure the runs of the code between them.
pub fn slang_invocations() -> u64 {
    SLANG_INVOCATIONS.load(Ordering::Relaxed)
}
