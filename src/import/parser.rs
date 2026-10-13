// SPDX-License-Identifier: Apache-2.0

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::import::parameterize::plain_import_parameters;
use crate::import::parser_cfg::ParserConfig;
use crate::import::specialize::SpecializationKey;
use crate::import::typed::{self, ImportedPort};
use crate::model::parameter::{ParameterSpec, ParameterType};
use crate::{ImportError, ModDef, ModDefCore, Span, Usage};

/// `(name, value)` pairs as owned strings.
fn owned_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// S0104 if the definition of `module`, elaborated alone, has interface
/// ports; `None` if it has none or does not elaborate alone either.
fn interface_port_error_of_definition(module: &str, cfg: &ParserConfig) -> Option<ImportError> {
    typed::interface_ports_of_definition(module, cfg)
        .filter(|ports| !ports.is_empty())
        .map(|ports| typed::interface_port_error(module, &ports))
}

/// The parser configuration a module was imported with, kept to rebuild a
/// configuration for later elaborations (parameterization). `tops` and
/// `include_hierarchy` are not kept: they are chosen per elaboration.
/// `parameters` (Slang `-G`) are kept and applied as overrides of every
/// specialization, not as `-G`.
#[derive(Clone)]
pub(crate) struct VerilogImport {
    /// Absolute source paths with `.` and `..` folded, in compile order.
    pub(crate) sources: Vec<String>,
    /// Absolute, like `sources`.
    pub(crate) incdirs: Vec<String>,
    pub(crate) defines: Vec<(String, String)>,
    /// Absolute, like `sources`.
    pub(crate) libfiles: Vec<String>,
    /// Absolute, like `sources`.
    pub(crate) libdirs: Vec<String>,
    pub(crate) libexts: Vec<String>,
    pub(crate) ignore_unknown_modules: bool,
    pub(crate) ignore_protected: bool,
    pub(crate) timescale: Option<String>,
    pub(crate) skip_unsupported: bool,
    pub(crate) extra_arguments: Vec<String>,
    /// `ParserConfig::parameters`: base overrides of every specialization,
    /// which explicit overrides replace.
    pub(crate) parameters: Vec<(String, String)>,
    /// Set on a specialization ([`ModDef::try_parameterize`]): the identity of
    /// the module and overrides it was elaborated with.
    pub(crate) specialization: Option<SpecializationKey>,
    /// The sources are a temporary file written from text passed to
    /// [`ModDef::from_verilog`]; it is deleted once the import returns, so
    /// the build manifest leaves it out.
    pub(crate) inline: bool,
}

impl VerilogImport {
    /// Calls `f` with a parser configuration for these sources, the given
    /// `tops`, `extra_sources` appended to the sources and `extra_arguments`
    /// appended to the extra arguments.
    pub(crate) fn with_config<R>(
        &self,
        tops: &[&str],
        extra_sources: &[&str],
        extra_arguments: &[&str],
        f: impl FnOnce(&ParserConfig) -> R,
    ) -> R {
        fn strs(items: &[String]) -> Vec<&str> {
            items.iter().map(String::as_str).collect()
        }
        let mut sources = strs(&self.sources);
        sources.extend_from_slice(extra_sources);
        let mut arguments = strs(&self.extra_arguments);
        arguments.extend_from_slice(extra_arguments);
        let defines: Vec<(&str, &str)> = self
            .defines
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        f(&ParserConfig {
            sources: &sources,
            tops,
            incdirs: &strs(&self.incdirs),
            defines: &defines,
            parameters: &[],
            libfiles: &strs(&self.libfiles),
            libdirs: &strs(&self.libdirs),
            libexts: &strs(&self.libexts),
            ignore_unknown_modules: self.ignore_unknown_modules,
            ignore_protected: self.ignore_protected,
            timescale: self.timescale.as_deref(),
            skip_unsupported: self.skip_unsupported,
            include_hierarchy: false,
            extra_arguments: &arguments,
        })
    }

    pub(crate) fn new(cfg: &ParserConfig) -> Self {
        let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        let cwd = std::env::current_dir().unwrap_or_default();
        let absolute = |items: &[&str]| {
            items
                .iter()
                .map(|s| slang_rs::absolute_path(&cwd, s))
                .collect()
        };
        VerilogImport {
            sources: absolute(cfg.sources),
            incdirs: absolute(cfg.incdirs),
            defines: owned_pairs(cfg.defines),
            libfiles: absolute(cfg.libfiles),
            libdirs: absolute(cfg.libdirs),
            libexts: strings(cfg.libexts),
            ignore_unknown_modules: cfg.ignore_unknown_modules,
            ignore_protected: cfg.ignore_protected,
            timescale: cfg.timescale.map(str::to_string),
            skip_unsupported: cfg.skip_unsupported,
            extra_arguments: strings(cfg.extra_arguments),
            parameters: owned_pairs(cfg.parameters),
            specialization: None,
            inline: false,
        }
    }
}

pub(crate) fn parser_param_to_param(
    parser_param: &slang_rs::ParameterDef,
) -> Result<(String, ParameterType), String> {
    match &parser_param.ty {
        slang_rs::Type::Logic {
            signed,
            unpacked_dimensions,
            packed_dimensions,
        } => {
            if !unpacked_dimensions.is_empty() {
                return Err(
                    "Parameters with unpacked dimensions are not currently supported".to_string(),
                );
            }
            let width = parser_param.ty.width()?;
            // Packed arrays of signed integers should be treated as unsigned
            // TODO(zhemao): Proper support for packed array types
            let is_array = packed_dimensions.len() > 1;
            let param_type = if *signed && !is_array {
                ParameterType::Signed(width)
            } else {
                ParameterType::Unsigned(width)
            };
            Ok((parser_param.name.clone(), param_type))
        }
        _ => {
            // TODO(zhemao): Proper support for struct, union, and enum types
            // For now, we can just treat them as flat unsigned integers
            let width = parser_param.ty.width()?;
            Ok((parser_param.name.clone(), ParameterType::Unsigned(width)))
        }
    }
}

impl ModDef {
    fn mod_def_from_imported_ports(
        mod_def_name: &str,
        imported_ports: &[ImportedPort],
        parameters: IndexMap<String, ParameterSpec>,
        cfg: &ParserConfig,
    ) -> ModDef {
        let mod_def = ModDef {
            core: Arc::new(RwLock::new(ModDefCore {
                name: mod_def_name.to_string(),
                span: Span::None,
                ports: IndexMap::new(),
                port_types: IndexMap::new(),
                port_spans: IndexMap::new(),
                enum_ports: IndexMap::new(),
                interfaces: IndexMap::new(),
                instances: IndexMap::new(),
                usage: Usage::EmitNothingAndStop,
                verilog_import: Some(VerilogImport::new(cfg)),
                parameters,
                mod_inst_connections: IndexMap::new(),
                mod_def_connections: IndexMap::new(),
                mod_def_metadata: HashMap::new(),
                mod_def_port_metadata: HashMap::new(),
                mod_def_intf_metadata: HashMap::new(),
                mod_inst_metadata: HashMap::new(),
                mod_inst_port_metadata: HashMap::new(),
                mod_inst_intf_metadata: HashMap::new(),
                shape: None,
                keepout: None,
                layer: None,
                inst_placements: IndexMap::new(),
                physical_pins: IndexMap::new(),
                port_max_distances: IndexMap::new(),
                track_definitions: None,
                track_occupancies: None,
                default_connection_max_distance: Some(0),
                specified_net_names: HashSet::new(),
                pipeline_counter: 0..,
            })),
        };
        mod_def.core.write().insert_imported_ports(imported_ports);
        mod_def
    }

    /// Creates a new module definition from a Verilog file. The `name`
    /// parameter is the name of the module to extract from the Verilog file,
    /// and `verilog` is the path to the Verilog file. If
    /// `ignore_unknown_modules` is `true`, do not panic if the Verilog file
    /// instantiates modules whose definitions cannot be found. This is often
    /// useful because only the interface of module `name` needs to be
    /// extracted; its contents do not need to be interpreted. If
    /// `skip_unsupported` is `true`, do not panic if the interface of module
    /// `name` contains unsupported features; simply skip these ports. This is
    /// occasionally useful when prototyping.
    pub fn from_verilog_file(
        name: impl AsRef<str>,
        verilog: &Path,
        ignore_unknown_modules: bool,
        skip_unsupported: bool,
    ) -> Self {
        Self::from_verilog_files(name, &[verilog], ignore_unknown_modules, skip_unsupported)
    }

    /// Creates a new module definition from a list of Verilog files. The `name`
    /// parameter is the name of the module to extract from the Verilog sources,
    /// and `verilog` is an array of paths of Verilog sources. If
    /// `ignore_unknown_modules` is `true`, do not panic if the Verilog file
    /// instantiates modules whose definitions cannot be found. This is often
    /// useful because only the interface of module `name` needs to be
    /// extracted; its contents do not need to be interpreted. If
    /// `skip_unsupported` is `true`, do not panic if the interface of module
    /// `name` contains unsupported features; simply skip these ports. This is
    /// occasionally useful when prototyping.
    pub fn from_verilog_files(
        name: impl AsRef<str>,
        verilog: &[&Path],
        ignore_unknown_modules: bool,
        skip_unsupported: bool,
    ) -> Self {
        let cfg = ParserConfig {
            sources: &verilog
                .iter()
                .map(|path| path.to_str().unwrap())
                .collect::<Vec<_>>(),
            ignore_unknown_modules,
            skip_unsupported,
            ..Default::default()
        };

        Self::from_verilog_with_config(name, &cfg)
    }

    /// Creates a new module definition from Verilog source code. The `name`
    /// parameter is the name of the module to extract from the Verilog code,
    /// and `verilog` is a string containing Verilog code. If
    /// `ignore_unknown_modules` is `true`, do not panic if the Verilog file
    /// instantiates modules whose definitions cannot be found. This is often
    /// useful because only the interface of module `name` needs to be
    /// extracted; its contents do not need to be interpreted. If
    /// `skip_unsupported` is `true`, do not panic if the interface of module
    /// `name` contains unsupported features; simply skip these ports. This is
    /// occasionally useful when prototyping.
    pub fn from_verilog(
        name: impl AsRef<str>,
        verilog: impl AsRef<str>,
        ignore_unknown_modules: bool,
        skip_unsupported: bool,
    ) -> Self {
        let verilog = slang_rs::str2tmpfile(verilog.as_ref()).unwrap();

        let cfg = ParserConfig {
            sources: &[verilog.path().to_str().unwrap()],
            ignore_unknown_modules,
            skip_unsupported,
            ..Default::default()
        };

        let mod_def = Self::from_verilog_with_config(name, &cfg);
        if let Some(import) = mod_def.core.write().verilog_import.as_mut() {
            import.inline = true;
        }
        mod_def
    }

    /// Creates a new module definition from Verilog sources. The `name`
    /// parameter is the name of the module to extract from Verilog code, and
    /// `cfg` is a `ParserConfig` struct specifying source files, include
    /// directories, etc. Panics with the rendered diagnostics if the import
    /// fails; see [`ModDef::try_from_verilog_with_config`].
    #[track_caller]
    pub fn from_verilog_with_config(name: impl AsRef<str>, cfg: &ParserConfig) -> Self {
        Self::try_from_verilog_with_config(name.as_ref(), cfg).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Fallible form of [`ModDef::from_verilog_with_config`]. Ports keep their
    /// resolved types ([`Port::port_type`](crate::Port::port_type)) and
    /// declaration spans ([`Port::source_span`](crate::Port::source_span)).
    ///
    /// Errors:
    /// - S0102 if Slang fails to elaborate the sources, or `name` is not a
    ///   top-level module of the elaborated design.
    /// - S0103 for ports with unpacked dimensions or a type that is not
    ///   integral (`real`, `string`, `event`, ...), unless
    ///   `cfg.skip_unsupported` is set (the ports are then skipped).
    /// - S0104 for interface ports. A module with interface ports does not
    ///   elaborate as a top, so these are found by elaborating the definition
    ///   alone with `--allow-toplevel-iface-ports`. If the sources elaborate
    ///   anyway (that flag in `cfg.extra_arguments`) and `cfg.skip_unsupported`
    ///   is set, the interface ports are skipped.
    /// - S0105 for a Slang `-G` value (`cfg.parameters`) of a parameter the
    ///   module declares that cannot be emitted as an override: it must be a
    ///   decimal integer, a string literal or a name, and set a value
    ///   parameter. Instances of the module are emitted with these
    ///   overrides, which its ports were resolved with; `-G` values of
    ///   parameters it does not declare are ignored, as by Slang. The Rust
    ///   span points at the caller.
    #[track_caller]
    pub fn try_from_verilog_with_config(
        name: &str,
        cfg: &ParserConfig,
    ) -> Result<Self, ImportError> {
        let caller = Span::caller();
        let output = match slang_rs::run_slang(&cfg.to_slang_config()) {
            Ok(output) => output,
            Err(e) => {
                return Err(interface_port_error_of_definition(name, cfg)
                    .unwrap_or_else(|| typed::slang_failure(name, &e)));
            }
        };

        let Some(imported_ports) =
            typed::import_top_ports(&output.design, name, cfg.skip_unsupported)
        else {
            return Err(interface_port_error_of_definition(name, cfg)
                .unwrap_or_else(|| typed::module_not_found(name)));
        };
        let imported_ports = imported_ports?;
        let parameters =
            plain_import_parameters(name, &owned_pairs(cfg.parameters), &output.design, &caller)?;

        let mod_def = Self::mod_def_from_imported_ports(name, &imported_ports, parameters, cfg);

        if cfg.include_hierarchy {
            let mod_def_with_hierarchy = mod_def.stub(name);
            let hierarchy = output.design.hierarchy();
            if let Some(inst) = hierarchy.get(name) {
                crate::import::hierarchy::populate_hierarchy(&mod_def_with_hierarchy, inst);
            }
            Ok(mod_def_with_hierarchy)
        } else {
            Ok(mod_def)
        }
    }

    /// Imports every top-level module, as [`ModDef::from_verilog_with_config`]
    /// does one. Panics with the rendered diagnostics on error.
    #[track_caller]
    pub fn all_from_verilog_with_config(cfg: &ParserConfig) -> Vec<Self> {
        let caller = Span::caller();
        let parameters = owned_pairs(cfg.parameters);
        let output = slang_rs::run_slang(&cfg.to_slang_config()).unwrap();
        // Every top-level module (with or without interface ports), sorted by
        // name for a deterministic order.
        let mut top_names: Vec<String> = output.design.tops().map(str::to_string).collect();
        top_names.sort();
        top_names.dedup();

        let mod_defs: Vec<ModDef> = top_names
            .into_iter()
            .map(|name| {
                let imported_ports =
                    typed::import_top_ports(&output.design, &name, cfg.skip_unsupported)
                        .expect("listed as a top-level module")
                        .unwrap_or_else(|e| panic!("{e}"));
                let parameters =
                    plain_import_parameters(&name, &parameters, &output.design, &caller)
                        .unwrap_or_else(|e| panic!("{e}"));
                Self::mod_def_from_imported_ports(&name, &imported_ports, parameters, cfg)
            })
            .collect();

        if cfg.include_hierarchy {
            let mut mod_defs_with_hierarchy = Vec::new();
            let hierarchy = output.design.hierarchy();
            for mod_def in mod_defs.iter() {
                let mod_def_name = mod_def.get_name();
                if let Some(inst) = hierarchy.get(&mod_def_name) {
                    let stubbed_mod_def = mod_def.stub(&mod_def_name);
                    crate::import::hierarchy::populate_hierarchy(&stubbed_mod_def, inst);
                    mod_defs_with_hierarchy.push(stubbed_mod_def);
                }
            }
            mod_defs_with_hierarchy
        } else {
            mod_defs
        }
    }
}
