// SPDX-License-Identifier: Apache-2.0

//! The build manifest of an emitted design.

use std::collections::HashSet;
use std::path::PathBuf;

use parking_lot::RwLock;
use std::sync::Arc;

use crate::diagnostic::codes;
use crate::{Diagnostic, Diagnostics, Label, ModDef, ModDefCore, Severity, Span, Usage};

/// What the RTL emitted by [`ModDef::emit`] needs besides itself: the
/// sources of the imported modules it instantiates without emitting them,
/// and the specialization of each specialized instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Absolute paths (made absolute, with `.` and `..` folded, at import),
    /// deduplicated, in first-seen order: a depth-first walk of the
    /// hierarchy in instance order, each import's sources in its compile
    /// order.
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

/// An imported module whose definition is emitted, with the span of its
/// first port declaration (which locates the file that defines it).
struct EmittedImport {
    name: String,
    span: Span,
}

impl ModDef {
    /// The manifest of the RTL that [`ModDef::emit`] produces for this
    /// module. Panics with the rendered diagnostics if
    /// [`ModDef::try_manifest`] fails.
    pub fn manifest(&self) -> Manifest {
        self.try_manifest()
            .unwrap_or_else(|diagnostics| panic!("{}", diagnostics.render()))
    }

    /// Fallible form of [`ModDef::manifest`]. Reports S0301 for each
    /// imported module whose definition is emitted (re-emitted or stubbed)
    /// while the file that defines it is also a listed source: the emitted
    /// RTL and the sources would define it twice.
    ///
    /// The defining file is taken to be the file of the module's first port
    /// declaration that has an SV span. Not checked: modules without ports,
    /// modules none of whose ports has an SV span, and modules whose first
    /// such port is declared in an `include`d file (the included file is
    /// compared instead of the module's).
    pub fn try_manifest(&self) -> Result<Manifest, Diagnostics> {
        let mut manifest = Manifest::default();
        let mut seen = HashSet::new();
        let mut emitted = Vec::new();
        let name = self.core.read().name.clone();
        visit(&self.core, &name, &mut manifest, &mut seen, &mut emitted);

        let diagnostics: Vec<Diagnostic> = emitted
            .into_iter()
            .filter_map(|module| {
                let Span::Sv { file, .. } = &module.span else {
                    return None;
                };
                // Slang reports absolute paths, folded like the sources.
                let file = PathBuf::from(file);
                seen.contains(&file).then(|| Diagnostic {
                    code: codes::S0301,
                    severity: Severity::Error,
                    message: format!(
                        "module `{}` would be defined twice: by the emitted RTL and by `{}`",
                        module.name,
                        file.display()
                    ),
                    primary: Label {
                        span: module.span.clone(),
                        text: format!("`{}` is defined in this listed source", module.name),
                    },
                    secondary: vec![],
                    notes: vec![format!(
                        "`{}` is emitted from the model, and its file is listed for other modules",
                        module.name
                    )],
                    helps: vec![
                        "move the module into its own file, or emit nothing for it \
                         (`Usage::EmitNothingAndStop`) and build with its source"
                            .into(),
                    ],
                })
            })
            .collect();
        if diagnostics.is_empty() {
            Ok(manifest)
        } else {
            Err(Diagnostics(diagnostics))
        }
    }
}

fn visit(
    core: &Arc<RwLock<ModDefCore>>,
    path: &str,
    manifest: &mut Manifest,
    seen: &mut HashSet<PathBuf>,
    emitted: &mut Vec<EmittedImport>,
) {
    let core = core.read();
    if let Some(import) = &core.verilog_import {
        if let Some(key) = &import.specialization {
            manifest
                .specializations
                .push((path.to_string(), key.short_hash()));
        }
        let is_emitted = matches!(
            core.usage,
            Usage::EmitDefinitionAndDescend | Usage::EmitStubAndStop
        );
        if is_emitted {
            let span = core
                .port_spans
                .values()
                .find(|span| matches!(span, Span::Sv { .. }));
            if let Some(span) = span
                && !emitted.iter().any(|m| m.name == core.name)
            {
                emitted.push(EmittedImport {
                    name: core.name.clone(),
                    span: span.clone(),
                });
            }
        } else if !import.inline {
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
            visit(
                inst,
                &format!("{path}.{inst_name}"),
                manifest,
                seen,
                emitted,
            );
        }
    }
}
