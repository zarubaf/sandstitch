// SPDX-License-Identifier: Apache-2.0

//! The reference NoC workloads shared by `bench.rs` (time and memory) and
//! `budgets.rs` (counts).

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use floonoc::*;
use topstitch::{ModDef, slang_invocations};

/// Peak resident set size of this process in KiB (`VmHWM`).
pub fn peak_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .expect("VmHWM in /proc/self/status");
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// `spec` with every core's seed replaced by `seed`.
pub fn with_one_seed(spec: &MeshSpec, seed: u32) -> MeshSpec {
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
pub fn build(
    cfg: &FlooConfig,
    spec: &MeshSpec,
    unique: bool,
    dir: &Path,
) -> (ModDef, Duration, u64) {
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
pub fn distinct(specializations: &[(String, String)], kind: impl Fn(&str) -> bool) -> usize {
    specializations
        .iter()
        .filter(|(path, _)| kind(path.rsplit('.').next().unwrap()))
        .map(|(_, hash)| hash.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

/// The spec of a workload: all cores share one seed (repeated) or keep
/// their distinct seeds (unique).
pub fn workload_spec(name: &str, spec: MeshSpec, unique: bool) -> MeshSpec {
    if unique {
        let seeds: Vec<u32> = spec.cores().iter().map(|&(_, _, t)| t.seed).collect();
        assert_eq!(
            seeds.iter().collect::<BTreeSet<_>>().len(),
            seeds.len(),
            "{name}: the cores need distinct seeds"
        );
        spec
    } else {
        with_one_seed(&spec, 1)
    }
}

/// The result of building one workload twice.
pub struct Measured {
    pub top: ModDef,
    pub cold: Duration,
    pub cold_slang: u64,
    pub warm: Duration,
    pub warm_slang: u64,
    /// Distinct router, chimney and core specializations.
    pub routers: usize,
    pub chimneys: usize,
    pub cores: usize,
}

/// Build the workload twice into one fresh directory (so the configuration
/// packages, and so the specialization keys, are equal) and count its
/// specializations.
pub fn measure(name: &str, (cfg, spec): (FlooConfig, MeshSpec), unique: bool) -> Measured {
    let spec = workload_spec(name, spec, unique);
    let dir = tempfile::tempdir().unwrap();
    let (top, cold, cold_slang) = build(&cfg, &spec, unique, dir.path());
    let (_, warm, warm_slang) = build(&cfg, &spec, unique, dir.path());
    let specializations = top.manifest().specializations;
    Measured {
        routers: distinct(&specializations, |n| n.starts_with("i_router_")),
        chimneys: distinct(&specializations, |n| n == "i_chimney"),
        cores: distinct(&specializations, |n| n.starts_with("i_core")),
        top,
        cold,
        cold_slang,
        warm,
        warm_slang,
    }
}
