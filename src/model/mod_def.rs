// SPDX-License-Identifier: Apache-2.0

use parking_lot::RwLock;

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::{Intf, MetadataKey, MetadataValue, Port, Span, Usage};

mod core;
pub use core::ModDefCore;

mod feedthrough;
mod hierarchy;
mod instances;
mod intf;
mod ports;
mod stub;
mod wrap;

/// Represents a module definition, like `module <mod_def_name> ... endmodule`
/// in Verilog.
#[derive(Clone)]
pub struct ModDef {
    pub(crate) core: Arc<RwLock<ModDefCore>>,
}

impl ModDef {
    /// Creates a new module definition with the given name.
    #[track_caller]
    pub fn new(name: impl AsRef<str>) -> ModDef {
        ModDef {
            core: Arc::new(RwLock::new(ModDefCore {
                name: name.as_ref().to_string(),
                span: Span::caller(),
                ports: IndexMap::new(),
                port_types: IndexMap::new(),
                port_spans: IndexMap::new(),
                enum_ports: IndexMap::new(),
                interfaces: IndexMap::new(),
                instances: IndexMap::new(),
                usage: Default::default(),
                verilog_import: None,
                stub_sources: None,
                parameters: IndexMap::new(),
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
        }
    }

    fn frozen(&self) -> bool {
        self.core.read().verilog_import.is_some()
    }

    /// Returns the name of this module definition.
    pub fn get_name(&self) -> String {
        self.core.read().name.clone()
    }

    /// Configures how this module definition should be used when validating
    /// and/or emitting Verilog.
    pub fn set_usage(&self, usage: Usage) {
        self.core.write().usage = usage;
    }

    /// Returns the `Usage` of this module definition.
    pub fn get_usage(&self) -> Usage {
        self.core.read().usage.clone()
    }

    pub fn set_metadata(
        &self,
        key: impl Into<MetadataKey>,
        value: impl Into<MetadataValue>,
    ) -> Self {
        self.core
            .write()
            .mod_def_metadata
            .insert(key.into(), value.into());
        self.clone()
    }

    pub fn get_metadata(&self, key: impl AsRef<str>) -> Option<MetadataValue> {
        self.core.read().mod_def_metadata.get(key.as_ref()).cloned()
    }

    pub fn clear_metadata(&self, key: impl AsRef<str>) -> Self {
        self.core.write().mod_def_metadata.remove(key.as_ref());
        self.clone()
    }
}

/// Indicates that a type can be converted to a `ModDef`. `ModDef` and
/// `ModInst` both implement this trait, which makes it easier to perform the
/// same operations on both.
pub trait ConvertibleToModDef {
    fn to_mod_def(&self) -> ModDef;
    fn get_port(&self, name: impl AsRef<str>) -> Port;
    fn get_intf(&self, name: impl AsRef<str>) -> Intf;
}

impl ConvertibleToModDef for ModDef {
    fn to_mod_def(&self) -> ModDef {
        self.clone()
    }
    fn get_port(&self, name: impl AsRef<str>) -> Port {
        self.get_port(name)
    }
    fn get_intf(&self, name: impl AsRef<str>) -> Intf {
        self.get_intf(name)
    }
}
