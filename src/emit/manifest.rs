// SPDX-License-Identifier: Apache-2.0

//! The build manifest of an emitted design.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use std::sync::Arc;

use crate::{ModDef, ModDefCore};

/// What emitted RTL needs besides itself: the sources of every imported
/// module it instantiates, and the specialization of each specialized
/// instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Absolute paths, deduplicated, in first-seen order: a depth-first walk
    /// of the hierarchy in instance order, each import's sources in its
    /// compile order.
    pub sources: Vec<PathBuf>,
    /// `(instance path, short hash)` for each instance of a specialization
    /// ([`ModDef::try_parameterize`]), in the same walk order. Instance
    /// paths start with the name of the module the manifest is made for
    /// (`top.u_router.u_fifo`).
    pub specializations: Vec<(String, String)>,
}

fn absolute(source: &str) -> PathBuf {
    let path = Path::new(source);
    std::fs::canonicalize(path).unwrap_or_else(|_| std::path::absolute(path).unwrap_or(path.into()))
}

impl ModDef {
    /// The manifest of this module and everything it instantiates.
    pub fn manifest(&self) -> Manifest {
        let mut manifest = Manifest::default();
        let mut seen = HashSet::new();
        let name = self.core.read().name.clone();
        visit(&self.core, &name, &mut manifest, &mut seen);
        manifest
    }
}

fn visit(
    core: &Arc<RwLock<ModDefCore>>,
    path: &str,
    manifest: &mut Manifest,
    seen: &mut HashSet<PathBuf>,
) {
    let core = core.read();
    if let Some(import) = &core.verilog_import {
        for source in &import.sources {
            let source = absolute(source);
            if seen.insert(source.clone()) {
                manifest.sources.push(source);
            }
        }
        if let Some(key) = &import.specialization {
            manifest
                .specializations
                .push((path.to_string(), key.short_hash()));
        }
    }
    for (inst_name, inst) in &core.instances {
        visit(inst, &format!("{path}.{inst_name}"), manifest, seen);
    }
}
