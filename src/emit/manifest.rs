// SPDX-License-Identifier: Apache-2.0

//! The build manifest of an emitted design.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use parking_lot::RwLock;
use std::sync::Arc;

use super::verilog::{EmitOptions, emitted_uses};
use crate::diagnostic::codes;
use crate::import::parser::VerilogImport;
use crate::resolve::design::resolve_design;
use crate::{
    Diagnostic, Diagnostics, Label, ModDef, ModDefCore, PortType, QualifiedName, Severity, Span,
    Usage,
};

/// What the RTL emitted by [`ModDef::emit`] needs besides itself: the
/// sources of the imported modules it instantiates without emitting them,
/// the files declaring the packages it uses, and the specialization of each
/// specialized instance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Absolute paths (made absolute, with `.` and `..` folded, at import),
    /// deduplicated, in first-seen order: a depth-first walk of the
    /// hierarchy in instance order, each import's sources in its compile
    /// order.
    ///
    /// The walk follows emission: only modules descended into are visited,
    /// and each lists the sources of its import. A module whose definition
    /// is emitted ([`Usage::EmitDefinitionAndDescend`],
    /// [`Usage::EmitStubAndStop`]) lists none, since its emitted definition
    /// would clash with the imported one, unless its import declares a
    /// package that the emitted RTL uses (in a typed port or net
    /// declaration, an enum cast or a parameter override) and no import
    /// listed already does. It then lists the sources of its import except
    /// the files defining modules whose definitions are emitted, and its
    /// include directories, libraries and definitions below. Text passed to
    /// [`ModDef::from_verilog`] lives in a temporary file deleted after
    /// import and is not listed: import such modules from files to build the
    /// emitted RTL with them.
    pub sources: Vec<PathBuf>,
    /// The include directories of the same imports, absolute (made absolute
    /// at import), deduplicated, in first-seen order.
    pub incdirs: Vec<PathBuf>,
    /// The library files (Slang `-v`) of the same imports, absolute,
    /// deduplicated, in first-seen order.
    pub libfiles: Vec<PathBuf>,
    /// The library directories (Slang `-y`) of the same imports, absolute,
    /// deduplicated, in first-seen order.
    pub libdirs: Vec<PathBuf>,
    /// The library file extensions (Slang `-Y`) of the same imports,
    /// deduplicated, in first-seen order.
    pub libexts: Vec<String>,
    /// The macro definitions (`ParserConfig::defines`, `(name, value)`) of
    /// the same imports, deduplicated, in first-seen order. Two imports
    /// defining one macro differently are an error (S0302).
    pub defines: Vec<(String, String)>,
    /// `(instance path, short hash)` for each instance of a specialization
    /// ([`ModDef::try_parameterize`]), in the same walk order. Instance
    /// paths start with the name of the module the manifest is made for
    /// (`top.u_router.u_fifo`).
    pub specializations: Vec<(String, String)>,
}

/// Where a listed macro definition came from.
struct DefineOrigin {
    value: String,
    module: String,
}

/// The imported modules a walk visits, each once, in walk order.
#[derive(Default)]
struct Walk {
    seen: HashSet<*const RwLock<ModDefCore>>,
    visited: Vec<Arc<RwLock<ModDefCore>>>,
}

/// A visited imported module.
struct Imported {
    name: String,
    /// Its definition is emitted (re-emitted or stubbed).
    emitted: bool,
    import: VerilogImport,
    /// The span of its first port declaration with an SV span, which
    /// locates the file defining it.
    span: Option<Span>,
}

impl Imported {
    /// The file defining the module, if known.
    fn defining_file(&self) -> Option<&str> {
        match &self.span {
            Some(Span::Sv { file, .. }) => Some(file),
            _ => None,
        }
    }
}

/// Where the emitted RTL first uses a package: a port declared with a type
/// from it, with the port's declaration span.
type Site = Option<(String, Span)>;

/// The manifest being listed and the diagnostics found so far.
#[derive(Default)]
struct Listing {
    manifest: Manifest,
    seen_sources: HashSet<PathBuf>,
    defines: IndexMap<String, DefineOrigin>,
    diagnostics: Vec<Diagnostic>,
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
    /// RTL and the sources would define it twice. Reports S0302 for each
    /// macro that two listed imports define with different values. Reports
    /// S0303 for each package the emitted RTL uses that the manifest cannot
    /// list: declared by no visited import, in a file defining an emitted
    /// module, or in a file that is neither a source nor a library of its
    /// import, or declared only by an emitted import whose defining file is
    /// not one of its sources.
    ///
    /// The defining file is taken to be the file of the module's first port
    /// declaration that has an SV span. Not checked by S0301: modules
    /// without ports, modules none of whose ports has an SV span, and
    /// modules whose first such port is declared in an `include`d file (the
    /// included file is compared instead of the module's). Which
    /// declarations are typed is decided as by [`EmitOptions::default`].
    pub fn try_manifest(&self) -> Result<Manifest, Diagnostics> {
        let design = resolve_design(self);
        let mut listing = Listing::default();
        let mut walk = Walk::default();
        let name = self.core.read().name.clone();
        visit(&self.core, &name, &mut listing.manifest, &mut walk);
        let imports: Vec<Imported> = walk
            .visited
            .iter()
            .map(|core| {
                let core = core.read();
                Imported {
                    name: core.name.clone(),
                    emitted: matches!(
                        core.usage,
                        Usage::EmitDefinitionAndDescend | Usage::EmitStubAndStop
                    ),
                    import: core
                        .verilog_import
                        .clone()
                        .expect("only imports are visited"),
                    span: core
                        .port_spans
                        .values()
                        .find(|span| matches!(span, Span::Sv { .. }))
                        .cloned(),
                }
            })
            .collect();

        // The packages the emitted RTL uses, with where it first uses them.
        let mut needed: IndexMap<String, Site> = IndexMap::new();
        for core in design.emitted() {
            let guard = core.read();
            // A definition with connection errors cannot be emitted (`check`
            // reports them), so it uses no package.
            if guard.usage != Usage::EmitStubAndStop && !design.resolved_cleanly(core) {
                continue;
            }
            let (uses, names) = emitted_uses(core, &guard, &design, &EmitOptions::default());
            for type_use in uses {
                let mut packages = Vec::new();
                type_packages(&type_use.ty, &mut packages);
                for package in packages {
                    needed
                        .entry(package)
                        .or_insert_with(|| type_use.port.clone());
                }
            }
            for name in names {
                if let Some((package, _)) = name.split_once("::") {
                    needed.entry(package.to_string()).or_insert(None);
                }
            }
        }

        // The files defining emitted imported modules, with those modules.
        let mut excluded: IndexMap<&str, &Imported> = IndexMap::new();
        for imported in imports.iter().filter(|i| i.emitted) {
            if let Some(file) = imported.defining_file() {
                excluded.entry(file).or_insert(imported);
            }
        }

        let to_list = choose_imports(&imports, &needed, &excluded, &mut listing.diagnostics);

        for (index, imported) in imports.iter().enumerate() {
            if imported.import.inline {
                continue;
            }
            if !imported.emitted {
                for source in &imported.import.sources {
                    add_source(source, &mut listing);
                }
                add_search(&imported.import, &imported.name, &mut listing);
            } else if to_list.contains_key(&index) {
                for source in &imported.import.sources {
                    if !excluded.contains_key(source.as_str()) {
                        add_source(source, &mut listing);
                    }
                }
                add_search(&imported.import, &imported.name, &mut listing);
            }
        }

        let Listing {
            manifest,
            seen_sources: seen,
            diagnostics: listing_diagnostics,
            ..
        } = listing;
        let mut reported = HashSet::new();
        let mut diagnostics: Vec<Diagnostic> = imports
            .iter()
            .filter(|i| i.emitted && reported.insert(i.name.clone()))
            .filter_map(|module| {
                let span = module.span.as_ref()?;
                // Slang reports absolute paths, folded like the sources.
                let file = PathBuf::from(module.defining_file()?);
                seen.contains(&file).then(|| Diagnostic {
                    code: codes::S0301,
                    severity: Severity::Error,
                    message: format!(
                        "module `{}` would be defined twice: by the emitted RTL and by `{}`",
                        module.name,
                        file.display()
                    ),
                    primary: Label {
                        span: span.clone(),
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
                    object: None,
                })
            })
            .collect();
        diagnostics.extend(listing_diagnostics);
        if diagnostics.is_empty() {
            Ok(manifest)
        } else {
            Err(Diagnostics(diagnostics))
        }
    }
}

/// Records the specializations under `this`, at instance path `path`, and
/// the imported modules visited, following emission.
fn visit(this: &Arc<RwLock<ModDefCore>>, path: &str, manifest: &mut Manifest, walk: &mut Walk) {
    let core = this.read();
    if let Some(import) = &core.verilog_import {
        if let Some(key) = &import.specialization {
            manifest
                .specializations
                .push((path.to_string(), key.short_hash()));
        }
        if walk.seen.insert(Arc::as_ptr(this)) {
            walk.visited.push(this.clone());
        }
    }
    if core.usage == Usage::EmitDefinitionAndDescend {
        for (inst_name, inst) in &core.instances {
            visit(inst, &format!("{path}.{inst_name}"), manifest, walk);
        }
    }
}

/// Chooses, for each package in `needed`, an import whose sources declare
/// it: a descended import, whose sources are listed anyway, or else an
/// emitted one, whose sources are then listed without the `excluded`
/// files (each with the emitted module it defines). Returns the indices of
/// the emitted imports chosen, each with the first package it is chosen
/// for; reports S0303 for each package no import can list, and for each
/// package of a chosen import declared in an excluded file.
fn choose_imports(
    imports: &[Imported],
    needed: &IndexMap<String, Site>,
    excluded: &IndexMap<&str, &Imported>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<usize, String> {
    let mut chosen = BTreeMap::new();
    for (package, site) in needed {
        let mut candidates: Vec<usize> = (0..imports.len())
            .filter(|&i| {
                !imports[i].import.inline && imports[i].import.package_files.contains_key(package)
            })
            .collect();
        // Descended imports first: their sources are listed anyway.
        candidates.sort_by_key(|&i| imports[i].emitted);
        let Some(&first) = candidates.first() else {
            push(diagnostics, undeclared(package, site));
            continue;
        };
        match candidates
            .iter()
            .find(|&&i| unlistable(&imports[i], package, site, excluded).is_none())
        {
            Some(&i) => {
                if imports[i].emitted {
                    chosen.entry(i).or_insert_with(|| package.clone());
                }
            }
            None => {
                let diagnostic = unlistable(&imports[first], package, site, excluded)
                    .expect("no candidate is listable");
                push(diagnostics, diagnostic);
            }
        }
    }
    // A chosen import's listed sources may use any package of its import:
    // one declared in an excluded file cannot be listed. (A needed package
    // is reported above.)
    for (&i, reason) in &chosen {
        for (package, file) in &imports[i].import.package_files {
            if needed.contains_key(package) {
                continue;
            }
            if let Some(owner) = excluded.get(file.as_str()) {
                let module = &owner.name;
                push(
                    diagnostics,
                    s0303(
                        format!(
                            "package `{package}` of the import listed for `{reason}` is \
                             declared in `{file}`, which defines emitted module `{module}`"
                        ),
                        Label {
                            span: owner.span.clone().unwrap_or(Span::None),
                            text: format!("`{module}` is defined in this file"),
                        },
                        format!(
                            "the sources listed for `{reason}` may use `{package}`, and \
                             `{module}` is emitted from the model, so the manifest leaves out \
                             its file"
                        ),
                    ),
                );
            }
        }
    }
    chosen
}

/// Adds `diagnostic` to `diagnostics` unless it is there already.
fn push(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    if !diagnostics.iter().any(|d| d.message == diagnostic.message) {
        diagnostics.push(diagnostic);
    }
}

/// Why the sources of `imported` cannot list `package`, if they cannot.
fn unlistable(
    imported: &Imported,
    package: &str,
    site: &Site,
    excluded: &IndexMap<&str, &Imported>,
) -> Option<Diagnostic> {
    let import = &imported.import;
    let file = &import.package_files[package];
    let module = &imported.name;
    if imported.emitted {
        let defining = imported.defining_file();
        if !defining.is_some_and(|f| import.sources.iter().any(|s| s == f)) {
            return Some(s0303(
                format!(
                    "the file defining emitted module `{module}` is not a source of its \
                     import, so the packages it needs cannot be listed"
                ),
                Label {
                    span: imported.span.clone().unwrap_or(Span::None),
                    text: format!("the first port of `{module}` is declared here"),
                },
                format!(
                    "`{module}` is emitted from the model, so the manifest lists the sources \
                     of its import except the file defining it"
                ),
            ));
        }
    }
    if let Some(owner) = excluded.get(file.as_str()) {
        return Some(in_module_file(package, &owner.name, file, site));
    }
    if !import.sources.contains(file) && !in_library(file, import) {
        let message = if imported.emitted {
            format!(
                "package `{package}` needed by emitted module `{module}` is declared in \
                 `{file}`, which is not a source or library of its import"
            )
        } else {
            format!(
                "package `{package}` used by the emitted RTL is declared in `{file}`, which \
                 is not a source or library of the import of `{module}`"
            )
        };
        return Some(s0303(
            message,
            site_label(package, site, imported.span.clone()),
            "the manifest lists only the sources and libraries of imports".to_string(),
        ));
    }
    None
}

/// S0303 for `package`, declared in `file`, which defines the emitted
/// module `module`.
fn in_module_file(package: &str, module: &str, file: &str, site: &Site) -> Diagnostic {
    s0303(
        format!(
            "package `{package}` needed by emitted module `{module}` is declared in the \
             module's own file `{file}`"
        ),
        site_label(package, site, None),
        format!(
            "`{module}` is emitted from the model, so the manifest leaves out its file, and \
             `{package}` with it"
        ),
    )
}

/// S0303 for `package`, which no visited import declares.
fn undeclared(package: &str, site: &Site) -> Diagnostic {
    s0303(
        format!(
            "package `{package}` used by the emitted RTL is declared by no import of the design"
        ),
        site_label(package, site, None),
        "the manifest lists the packages of the imports the emitted RTL instantiates or \
         emits; imports of text passed to `from_verilog` are not listed"
            .to_string(),
    )
}

/// The label of an S0303 about `package`: the port first using it, or else
/// `fallback`.
fn site_label(package: &str, site: &Site, fallback: Option<Span>) -> Label {
    match site {
        Some((port, span)) => Label {
            span: span.clone(),
            text: format!("`{port}` is declared with a type from `{package}`"),
        },
        None => Label {
            span: fallback.unwrap_or(Span::None),
            text: format!("the emitted RTL uses `{package}`"),
        },
    }
}

/// An S0303 diagnostic.
fn s0303(message: String, primary: Label, note: String) -> Diagnostic {
    Diagnostic {
        code: codes::S0303,
        severity: Severity::Error,
        message,
        primary,
        secondary: vec![],
        notes: vec![note],
        helps: vec![
            "declare the package in a file of its own and list it among the import's sources"
                .into(),
        ],
        object: None,
    }
}

/// Whether `file` is a library file of `import` or in one of its library
/// directories.
fn in_library(file: &str, import: &VerilogImport) -> bool {
    import.libfiles.iter().any(|f| f == file)
        || import
            .libdirs
            .iter()
            .any(|dir| Path::new(file).starts_with(dir))
}

/// Appends the packages that `ty`, its fields and its enum base name to
/// `packages`.
fn type_packages(ty: &PortType, packages: &mut Vec<String>) {
    let (name, fields) = match ty {
        PortType::Logic { .. } => return,
        PortType::Struct { name, fields, .. } | PortType::Union { name, fields, .. } => {
            (name, fields.as_slice())
        }
        PortType::Enum { name, base, .. } => {
            type_packages(base, packages);
            (name, [].as_slice())
        }
    };
    if let Some(QualifiedName {
        package: Some(package),
        ..
    }) = name
    {
        packages.push(package.clone());
    }
    for field in fields {
        type_packages(&field.ty, packages);
    }
}

/// Lists `source` unless it is listed already.
fn add_source(source: &str, listing: &mut Listing) {
    let source = PathBuf::from(source);
    if listing.seen_sources.insert(source.clone()) {
        listing.manifest.sources.push(source);
    }
}

/// Lists the include directories, library search and macro definitions of
/// `import`, the import of `module`.
fn add_search(import: &VerilogImport, module: &str, listing: &mut Listing) {
    let manifest = &mut listing.manifest;
    push_new(
        &mut manifest.incdirs,
        import.incdirs.iter().map(PathBuf::from),
    );
    push_new(
        &mut manifest.libfiles,
        import.libfiles.iter().map(PathBuf::from),
    );
    push_new(
        &mut manifest.libdirs,
        import.libdirs.iter().map(PathBuf::from),
    );
    push_new(&mut manifest.libexts, import.libexts.iter().cloned());
    for (name, value) in &import.defines {
        add_define(name, value, module, listing);
    }
}

/// Lists the macro definition `name=value` of the import of `module`, or
/// reports S0302 if a listed import defines `name` differently (once per
/// value).
fn add_define(name: &str, value: &str, module: &str, listing: &mut Listing) {
    let Some(first) = listing.defines.get(name) else {
        listing.defines.insert(
            name.to_string(),
            DefineOrigin {
                value: value.to_string(),
                module: module.to_string(),
            },
        );
        listing
            .manifest
            .defines
            .push((name.to_string(), value.to_string()));
        return;
    };
    if first.value == value {
        return;
    }
    let message = format!(
        "macro `{name}` is defined as `{}` for `{}` and as `{value}` for `{module}`",
        first.value, first.module
    );
    if listing.diagnostics.iter().any(|d| d.message == message) {
        return;
    }
    listing.diagnostics.push(Diagnostic {
        code: codes::S0302,
        severity: Severity::Error,
        message,
        primary: Label {
            span: Span::None,
            text: format!("`{module}` was imported with `{name}={value}`"),
        },
        secondary: vec![],
        notes: vec![format!(
            "`{}` was imported with `{name}={}`; the emitted RTL is built with one set of \
             definitions",
            first.module, first.value
        )],
        helps: vec![
            "import the modules with the same definition, or rename the macro in one of them"
                .into(),
        ],
        object: None,
    });
}

/// Appends the `items` not yet in `list`.
fn push_new<T: PartialEq>(list: &mut Vec<T>, items: impl IntoIterator<Item = T>) {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}
