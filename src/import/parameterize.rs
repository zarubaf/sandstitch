// SPDX-License-Identifier: Apache-2.0

use indexmap::IndexMap;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use num_bigint::BigInt;

use crate::diagnostic::codes;
use crate::import::parser::VerilogImport;
use crate::import::specialize::{self, SpecializationKey};
use crate::model::parameter::{ParameterSpec, ParameterValue};
use crate::{Diagnostics, ImportError, Label, ModDef, ModDefCore, ParserConfig, Span, Usage};

impl ModDef {
    /// Returns a new module definition that is a variant of this module
    /// definition, where the given parameters have been overridden from their
    /// default values. For example, if the module definition has a parameter
    /// `WIDTH` with a default value of `32`, calling `parameterize(&[("WIDTH",
    /// 64)])` will return a new module definition with the same name, whose
    /// ports are those of the module with `WIDTH` set to `64`. Instances of it
    /// are emitted as instances of the original module with a `#(...)`
    /// override; [`ModDef::wrap`] puts one in a wrapper module.
    ///
    /// Parameter values can be integers, strings, types
    /// ([`ParameterValue::Type`]) or named constants
    /// ([`ParameterValue::Expr`]). String values must contain only printable
    /// ASCII characters, including spaces, and cannot contain single quotes,
    /// double quotes, or backslashes. Mixed-type overrides can be supplied
    /// together as [`ParameterValue`], for example
    /// `parameterize::<ParameterValue>(&[("WIDTH", 32.into()),
    /// ("MODE", "FAST".into())])`.
    ///
    /// Overrides are merged with those of this module definition, if it is
    /// itself parameterized. Panics with the rendered diagnostics on error;
    /// see [`ModDef::try_parameterize`].
    #[track_caller]
    pub fn parameterize<T: Into<ParameterValue> + Clone>(
        &self,
        parameters: &[(&str, T)],
    ) -> ModDef {
        self.try_parameterize(parameters)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// Fallible form of [`ModDef::parameterize`]. The module is elaborated by
    /// Slang under a generated top that instantiates it with the overrides;
    /// the result is cached by its [`SpecializationKey`]
    /// ([`ModDef::specialization_key`]). Ports keep their resolved types.
    ///
    /// The `parameters` (Slang `-G`) of the configuration the module was
    /// imported with are base overrides of every specialization, so that
    /// they are not lost; explicit overrides replace them. A `-G` value must
    /// be a decimal integer, a string literal or a name.
    ///
    /// Errors (Rust spans point at the caller):
    /// - S0101 for an override of a parameter the module does not declare
    ///   (or a localparam).
    /// - S0102 if Slang fails to elaborate the specialization, for example an
    ///   unknown type; Slang's messages are notes.
    /// - S0103 for unsupported ports, as in
    ///   [`ModDef::try_from_verilog_with_config`].
    /// - S0104 if the module has interface ports.
    /// - S0105 for an unsupported override value: types and constants must
    ///   be `ident` or `pkg::ident`; strings as above; integers non-negative;
    ///   `-G` values as above.
    ///
    /// Panics if this module definition was not imported from Verilog.
    #[track_caller]
    pub fn try_parameterize<T: Into<ParameterValue> + Clone>(
        &self,
        parameters: &[(&str, T)],
    ) -> Result<ModDef, ImportError> {
        let caller = Span::caller();
        let core = self.core.read();
        let Some(verilog_import) = core.verilog_import.as_ref() else {
            panic!(
                "Error parameterizing {}: can only parameterize a module defined in external Verilog sources.",
                core.name
            );
        };

        // Merge parameter overrides with any existing ones.
        let mut overrides: IndexMap<String, ParameterValue> = core
            .parameters
            .iter()
            .map(|(k, v)| (k.clone(), v.value()))
            .collect();
        for (name, value) in parameters {
            overrides.insert(name.to_string(), value.clone().into());
        }

        Self::specialized(&core.name, verilog_import, overrides, &caller)
    }

    /// Imports module `name` from the sources in `cfg` directly with the
    /// parameter `overrides`, without elaborating it with its default
    /// parameters first. Use this for a module whose defaults do not
    /// elaborate, for example a type parameter defaulting to `logic` that the
    /// module uses as a struct, or a count defaulting to 0 that is used as an
    /// index. Otherwise the result is the same as
    /// [`ModDef::try_from_verilog_with_config`] followed by
    /// [`ModDef::try_parameterize`] with the same overrides: the same ports
    /// and types, [`ModDef::specialization_key`], emitted `#(...)` override
    /// and build manifest sources. It can be parameterized again; overrides
    /// merge as in [`ModDef::try_parameterize`].
    ///
    /// With no overrides this is [`ModDef::try_from_verilog_with_config`].
    /// Otherwise, as for any specialization, `cfg.tops` and
    /// `cfg.include_hierarchy` are not used: the module is elaborated under a
    /// generated top and the result has no instances.
    ///
    /// `cfg.parameters` (Slang `-G`) are base overrides, as in
    /// [`ModDef::try_parameterize`]; `overrides` replace them.
    ///
    /// Errors (Rust spans point at the caller) are those of
    /// [`ModDef::try_parameterize`].
    #[track_caller]
    pub fn try_from_verilog_parameterized<T: Into<ParameterValue> + Clone>(
        name: &str,
        cfg: &ParserConfig,
        overrides: &[(&str, T)],
    ) -> Result<ModDef, ImportError> {
        let caller = Span::caller();
        if overrides.is_empty() {
            return Self::try_from_verilog_with_config(name, cfg);
        }
        let overrides: IndexMap<String, ParameterValue> = overrides
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone().into()))
            .collect();
        Self::specialized(name, &VerilogImport::new(cfg), overrides, &caller)
    }

    /// Elaborates `name` from `verilog_import` with the import's Slang `-G`
    /// parameters, replaced by `overrides`.
    fn specialized(
        name: &str,
        verilog_import: &VerilogImport,
        overrides: IndexMap<String, ParameterValue>,
        caller: &Span,
    ) -> Result<ModDef, ImportError> {
        let mut merged = slang_parameter_overrides(name, verilog_import, &overrides, caller)?;
        merged.extend(overrides);
        let overrides = merged;
        specialize::validate(name, &overrides, caller)?;
        let specialized = specialize::specialize(name, verilog_import, &overrides, caller)?;

        let parameters: IndexMap<String, ParameterSpec> = overrides
            .into_iter()
            .map(|(name, value)| {
                let spec = match value {
                    ParameterValue::Integer(value) => ParameterSpec::Integer {
                        value,
                        ty: specialized.integer_parameter_types[&name].clone(),
                    },
                    ParameterValue::String(value) => ParameterSpec::String(value),
                    ParameterValue::Type(name) => ParameterSpec::Type(name),
                    ParameterValue::Expr(name) => ParameterSpec::Expr(name),
                };
                (name, spec)
            })
            .collect();

        let mut verilog_import = verilog_import.clone();
        verilog_import.specialization = Some(specialized.key.clone());

        let mod_def = ModDef {
            core: Arc::new(RwLock::new(ModDefCore {
                name: name.to_string(),
                ports: IndexMap::new(),
                port_types: IndexMap::new(),
                port_spans: IndexMap::new(),
                enum_ports: IndexMap::new(),
                interfaces: IndexMap::new(),
                instances: IndexMap::new(),
                usage: Usage::EmitNothingAndStop,
                verilog_import: Some(verilog_import),
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
        mod_def
            .core
            .write()
            .insert_imported_ports(&specialized.ports);
        Ok(mod_def)
    }

    /// The identity of this specialization, if this module definition was
    /// returned by [`ModDef::parameterize`] or [`ModDef::try_parameterize`].
    pub fn specialization_key(&self) -> Option<SpecializationKey> {
        self.core
            .read()
            .verilog_import
            .as_ref()?
            .specialization
            .clone()
    }
}

/// The import's Slang `-G` parameters not replaced by `explicit` as
/// overrides: a decimal integer, a string literal, or a name (`ident` or
/// `pkg::ident`, a named constant). S0105 for any other value.
fn slang_parameter_overrides(
    module: &str,
    import: &VerilogImport,
    explicit: &IndexMap<String, ParameterValue>,
    caller: &Span,
) -> Result<IndexMap<String, ParameterValue>, ImportError> {
    let mut overrides = IndexMap::new();
    let mut diagnostics = Vec::new();
    for (name, text) in &import.parameters {
        if explicit.contains_key(name) {
            continue;
        }
        let text = text.trim();
        let value = if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            Some(ParameterValue::Integer(
                text.parse::<BigInt>().expect("decimal digits"),
            ))
        } else if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
            Some(ParameterValue::String(text[1..text.len() - 1].to_string()))
        } else if specialize::NAME.is_match(text) {
            Some(ParameterValue::Expr(text.to_string()))
        } else {
            None
        };
        match value {
            Some(value) => {
                overrides.insert(name.clone(), value);
            }
            None => {
                let mut d = crate::import::typed::error(
                    codes::S0105,
                    format!("unsupported `-G` override for parameter `{name}` of `{module}`"),
                    Label {
                        span: caller.clone(),
                        text: format!(
                            "`ParserConfig::parameters` sets `{name}={text}`, which is not a \
                             decimal integer, string literal or name"
                        ),
                    },
                );
                d.helps.push(format!(
                    "override `{name}` explicitly; an explicit override replaces the `-G` value"
                ));
                diagnostics.push(d);
            }
        }
    }
    if diagnostics.is_empty() {
        Ok(overrides)
    } else {
        Err(ImportError {
            diagnostics: Diagnostics(diagnostics),
        })
    }
}
