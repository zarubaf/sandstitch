// SPDX-License-Identifier: Apache-2.0

//! The build manifest of an emitted design.

use std::collections::HashSet;
use std::path::PathBuf;

use parking_lot::RwLock;
use std::sync::Arc;

use crate::{ModDef, ModDefCore, Usage};

/// What the RTL emitted by [`ModDef::emit`] needs besides itself: the
/// sources of the imported modules it instantiates without emitting them,
/// and the specialization of each specialized instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Absolute paths (made absolute at import), deduplicated, in first-seen
    /// order: a depth-first walk of the hierarchy in instance order, each
    /// import's sources in its compile order.
    ///
    /// The walk follows emission: a module whose definition is emitted
    /// ([`Usage::EmitDefinitionAndDescend`], [`Usage::EmitStubAndStop`])
    /// contributes no sources, since its emitted definition would clash with
    /// the imported one; only modules descended into are visited. Text
    /// passed to [`ModDef::from_verilog`] lives in a temporary file deleted
    /// after import and is not listed: import such modules from files to
    /// build the emitted RTL with them.
    pub sources: Vec<PathBuf>,
    /// `(instance path, short hash)` for each instance of a specialization
    /// ([`ModDef::try_parameterize`]), in the same walk order. Instance
    /// paths start with the name of the module the manifest is made for
    /// (`top.u_router.u_fifo`).
    pub specializations: Vec<(String, String)>,
}

impl ModDef {
    /// The manifest of the RTL that [`ModDef::emit`] produces for this
    /// module.
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
        if let Some(key) = &import.specialization {
            manifest
                .specializations
                .push((path.to_string(), key.short_hash()));
        }
        let emitted = matches!(
            core.usage,
            Usage::EmitDefinitionAndDescend | Usage::EmitStubAndStop
        );
        if !emitted && !import.inline {
            for source in &import.sources {
                let source = PathBuf::from(source);
                if seen.insert(source.clone()) {
                    manifest.sources.push(source);
                }
            }
        }
    }
    if core.usage == Usage::EmitDefinitionAndDescend {
        for (inst_name, inst) in &core.instances {
            visit(inst, &format!("{path}.{inst_name}"), manifest, seen);
        }
    }
}
