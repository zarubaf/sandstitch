// SPDX-License-Identifier: Apache-2.0

use indexmap::IndexMap;
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::import::specialize::{self, SpecializationKey};
use crate::model::parameter::{ParameterSpec, ParameterValue};
use crate::{ImportError, ModDef, ModDefCore, Span, Usage};

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
    /// Errors (Rust spans point at the caller):
    /// - S0101 for an override of a parameter the module does not declare
    ///   (or a localparam).
    /// - S0102 if Slang fails to elaborate the specialization, for example an
    ///   unknown type; Slang's messages are notes.
    /// - S0103 for unsupported ports, as in
    ///   [`ModDef::try_from_verilog_with_config`].
    /// - S0104 if the module has interface ports.
    /// - S0105 for an unsupported override value: types and constants must
    ///   be `ident` or `pkg::ident`; strings as above; integers non-negative.
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

        specialize::validate(&core.name, &overrides, &caller)?;
        let specialized = specialize::specialize(&core.name, verilog_import, &overrides, &caller)?;

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
                name: core.name.clone(),
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
