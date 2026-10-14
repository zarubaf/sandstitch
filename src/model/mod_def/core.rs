// SPDX-License-Identifier: Apache-2.0

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::ops::RangeFrom;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::import::parser::VerilogImport;
use crate::physical::geometry::PhysicalPin;
use crate::physical::tracks::{TrackDefinitions, TrackOccupancies};

use crate::model::connection::PortSliceConnections;
use crate::model::intf::IntfMember;
use crate::{IO, Metadata, PortType, Span, Usage};

type PhysicalPinMap = IndexMap<String, Vec<Option<PhysicalPin>>>;
type MaxDistanceMap = IndexMap<String, Vec<Option<i64>>>;

/// Data structure representing a module definition.
///
/// Contains the module's name, ports, interfaces, instances, etc. Not intended
/// to be used directly; use `ModDef` instead, which contains a smart pointer to
/// this struct.
pub(crate) struct ModDefCore {
    pub(crate) name: String,
    /// Where the module definition was created in Rust; [`Span::None`] for
    /// imported and specialized definitions.
    pub(crate) span: Span,
    pub(crate) ports: IndexMap<String, IO>,
    /// Resolved types of typed ports, keyed like `ports`: ports imported
    /// from SystemVerilog and ports created by exporting a typed port
    /// (`export_as`, `wrap`). Added only by [`ModDefCore::insert_typed_port`],
    /// which keeps the `ports` width equal to the type's width; copied whole
    /// when a module definition is copied (`stub`, pin placement). Ports
    /// added without a type have no entry.
    pub(crate) port_types: IndexMap<String, PortType>,
    /// SystemVerilog declaration spans of imported ports, keyed like `ports`.
    pub(crate) port_spans: IndexMap<String, Span>,
    /// Interfaces by name; members in definition order.
    pub(crate) interfaces: IndexMap<String, Vec<IntfMember>>,
    pub(crate) instances: IndexMap<String, Arc<RwLock<ModDefCore>>>,
    pub(crate) usage: Usage,
    pub(crate) verilog_import: Option<VerilogImport>,
    /// Set on a [`ModDef::stub`] of an import: where its sources come from,
    /// for the build manifest only. It does not make the stub an import.
    pub(crate) stub_sources: Option<StubSources>,
    /// Parameter overrides applied to this module definition (by name)
    pub(crate) parameters: IndexMap<String, crate::model::parameter::ParameterSpec>,
    pub(crate) mod_inst_connections:
        IndexMap<String, IndexMap<String, Arc<RwLock<PortSliceConnections>>>>,
    pub(crate) mod_def_connections: IndexMap<String, Arc<RwLock<PortSliceConnections>>>,
    pub(crate) enum_ports: IndexMap<String, String>,
    pub(crate) mod_def_metadata: Metadata,
    pub(crate) mod_def_port_metadata: HashMap<String, Metadata>,
    pub(crate) mod_def_intf_metadata: HashMap<String, Metadata>,
    pub(crate) mod_inst_metadata: HashMap<String, Metadata>,
    pub(crate) mod_inst_port_metadata: HashMap<String, HashMap<String, Metadata>>,
    pub(crate) mod_inst_intf_metadata: HashMap<String, HashMap<String, Metadata>>,
    pub(crate) shape: Option<crate::physical::geometry::Polygon>,
    pub(crate) keepout: Option<crate::physical::geometry::Polygon>,
    pub(crate) layer: Option<String>,
    pub(crate) inst_placements: IndexMap<String, crate::physical::geometry::Placement>,
    pub(crate) physical_pins: PhysicalPinMap,
    pub(crate) port_max_distances: MaxDistanceMap,
    pub(crate) track_definitions: Option<TrackDefinitions>,
    pub(crate) track_occupancies: Option<TrackOccupancies>,
    pub(crate) default_connection_max_distance: Option<i64>,
    /// Set of net names explicitly specified via `specify_net_name` within
    /// this module definition. Used to detect duplicate specifications and to
    /// check for name collisions during emission.
    pub(crate) specified_net_names: HashSet<String>,
    /// Internal counter to generate unique pipeline instance names
    pub(crate) pipeline_counter: RangeFrom<usize>,
}

impl ModDefCore {
    /// Inserts (or replaces) a typed port: `ports[name]` gets direction `io`
    /// and the type's width, `port_types[name]` the type and `port_spans[name]`
    /// its declaration span. This is the only function that adds entries to
    /// `port_types`; copies of a module definition clone the map.
    pub(crate) fn insert_typed_port(
        &mut self,
        name: String,
        io: fn(usize) -> IO,
        ty: PortType,
        span: Span,
    ) {
        self.ports.insert(name.clone(), io(ty.width()));
        self.port_spans.insert(name.clone(), span);
        self.port_types.insert(name, ty);
    }

    /// The type of port `name`, if it has one.
    pub(crate) fn port_type(&self, name: &str) -> Option<&PortType> {
        let ty = self.port_types.get(name)?;
        debug_assert_eq!(
            self.ports.get(name).map(IO::width),
            Some(ty.width()),
            "port {}.{name}: IO width and type width disagree",
            self.name
        );
        Some(ty)
    }

    pub fn get_physical_pin(&self, port_name: &str, bit: usize) -> PhysicalPin {
        let pins = self.physical_pins.get(port_name).unwrap_or_else(|| {
            panic!(
                "Physical pins for port {}.{} not defined",
                self.name, port_name
            )
        });

        if bit >= pins.len() {
            panic!(
                "Bit {} out of range for port {}.{} (width {})",
                bit,
                self.name,
                port_name,
                pins.len()
            );
        }

        pins[bit]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "Physical pin for {}.{}[{}] is not placed",
                    self.name, port_name, bit
                )
            })
            .clone()
    }
}

/// The import a stub was made from, which the build manifest reads to list
/// the packages the stub's ports use.
#[derive(Clone)]
pub(crate) struct StubSources {
    pub(crate) import: VerilogImport,
    /// The name of the imported module, which a stub of a stub keeps comparing
    /// against.
    pub(crate) module: String,
    /// The stub has the module name of the import, so the file defining that
    /// module defines it too.
    pub(crate) defines_module: bool,
}
