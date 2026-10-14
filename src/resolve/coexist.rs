// SPDX-License-Identifier: Apache-2.0

//! Comparison of two definitions that share a [`DefKey`]: equal ones are one
//! definition, different ones are S0408. Spans and metadata are provenance
//! and are not compared.

use std::sync::Arc;

use indexmap::IndexMap;
use num_bigint::BigInt;
use parking_lot::RwLock;

use super::key::{DefKey, def_key};
use super::{NetName, NetSlice, Resolution, Source};
use crate::model::connection::connected_item::Wire;
use crate::model::intf::IntfMember;
use crate::model::parameter::ParameterSpec;
use crate::model::port_slice::path_string;
use crate::{IO, ModDefCore, Port, PortSlice, PortType, Select, Usage};

/// A span-free view of a definition, for comparison only.
pub(crate) struct DefView {
    usage: Usage,
    ports: Vec<(String, IO, Option<PortType>)>,
    enum_ports: IndexMap<String, String>,
    parameters: IndexMap<String, ParameterSpec>,
    instances: Vec<(String, DefKey)>,
    interfaces: Vec<(String, Vec<IntfMember>)>,
    /// The connections as made, `(end, end, bits)` with the two ends in
    /// [`SliceView::order`] and the list sorted, so that call order and
    /// direction do not count: whether a connection is type-checked depends
    /// on `bits`.
    records: Vec<(SliceView, SliceView, bool)>,
    /// Set only for `EmitDefinitionAndDescend`, from an error-free
    /// resolution.
    connections: Option<Connections>,
}

/// An end of a definition, named by instance and port name.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum EndName {
    Def(String),
    Inst(Vec<String>, String),
}

#[derive(PartialEq)]
struct SliceView {
    end: EndName,
    msb: usize,
    lsb: usize,
    select: Option<Arc<[Select]>>,
}

#[derive(PartialEq)]
enum SourceView {
    Port(SliceView),
    Wire(Wire),
    Constant(BigInt, usize),
    Unused,
}

/// The canonical form of a [`Resolution`].
#[derive(PartialEq)]
struct Connections {
    ends: Vec<(String, Vec<(SliceView, SourceView)>)>,
    nets: Vec<(NetName, usize)>,
}

impl DefView {
    /// The view of `core`, without its connections. Instances, interfaces
    /// and connection records are kept only for `EmitDefinitionAndDescend`:
    /// a stub is its interface whatever its internals.
    pub(crate) fn new(core: &Arc<RwLock<ModDefCore>>) -> Self {
        let core = core.read();
        let emitted = core.usage == Usage::EmitDefinitionAndDescend;
        let mut records: Vec<_> = if emitted {
            core.mod_def_connections
                .values()
                .chain(core.mod_inst_connections.values().flat_map(|p| p.values()))
                .flat_map(|c| {
                    c.read()
                        .records()
                        .iter()
                        .map(|r| {
                            let (this, other) = (SliceView::new(&r.this), SliceView::new(&r.other));
                            if this.order() <= other.order() {
                                (this, other, r.bits)
                            } else {
                                (other, this, r.bits)
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        } else {
            Vec::new()
        };
        records
            .sort_by(|a, b| (a.0.order(), a.1.order(), a.2).cmp(&(b.0.order(), b.1.order(), b.2)));
        DefView {
            records,
            usage: core.usage.clone(),
            ports: core
                .ports
                .iter()
                .map(|(name, io)| (name.clone(), io.clone(), core.port_types.get(name).cloned()))
                .collect(),
            enum_ports: core.enum_ports.clone(),
            parameters: core.parameters.clone(),
            instances: if emitted {
                core.instances
                    .iter()
                    .map(|(name, child)| (name.clone(), def_key(&child.read())))
                    .collect()
            } else {
                Vec::new()
            },
            interfaces: if emitted {
                core.interfaces
                    .iter()
                    .map(|(name, members)| (name.clone(), members.clone()))
                    .collect()
            } else {
                Vec::new()
            },
            connections: None,
        }
    }

    /// Adds the connections of an `EmitDefinitionAndDescend` definition
    /// from its error-free `resolution`.
    pub(crate) fn set_connections(&mut self, resolution: &Resolution) {
        debug_assert!(self.usage == Usage::EmitDefinitionAndDescend);
        self.connections = Some(Connections::new(resolution));
    }

    /// Whether the definition is `EmitDefinitionAndDescend`, so that its
    /// connections are compared.
    pub(crate) fn descends(&self) -> bool {
        self.usage == Usage::EmitDefinitionAndDescend
    }

    /// Whether the view has connections.
    pub(crate) fn has_connections(&self) -> bool {
        self.connections.is_some()
    }
}

/// Whether emission writes a definition of a module with `usage`.
pub(crate) fn is_emitted(usage: &Usage) -> bool {
    matches!(
        usage,
        Usage::EmitDefinitionAndDescend | Usage::EmitStubAndStop
    )
}

/// The first field in which `a` and `b` differ, in the order usage, ports,
/// parameter overrides, instances, interfaces, connections. Instances and
/// interfaces are compared only for `EmitDefinitionAndDescend`, connections
/// (resolutions and records) only when both views have them.
pub(crate) fn first_difference(a: &DefView, b: &DefView) -> Option<&'static str> {
    if a.usage != b.usage {
        return Some("usage");
    }
    if a.ports != b.ports || a.enum_ports != b.enum_ports {
        return Some("ports");
    }
    if a.parameters != b.parameters {
        return Some("parameter overrides");
    }
    if !a.descends() {
        return None;
    }
    if a.instances != b.instances {
        return Some("instances");
    }
    if a.interfaces != b.interfaces {
        return Some("interfaces");
    }
    match (&a.connections, &b.connections) {
        (Some(ac), Some(bc)) if ac != bc || a.records != b.records => Some("connections"),
        _ => None,
    }
}

impl Connections {
    fn new(resolution: &Resolution) -> Self {
        let ends = resolution
            .instances
            .iter()
            .flatten()
            .chain(&resolution.ports)
            .map(|end| {
                let segments = end
                    .segments
                    .iter()
                    .map(|segment| {
                        let source = match &segment.source {
                            Source::Net(NetSlice::Port(slice)) => {
                                SourceView::Port(SliceView::new(slice))
                            }
                            Source::Net(NetSlice::Wire(wire)) => SourceView::Wire(wire.clone()),
                            Source::Constant(tieoff) => {
                                SourceView::Constant(tieoff.value.clone(), tieoff.width)
                            }
                            Source::Unused => SourceView::Unused,
                        };
                        (SliceView::new(&segment.this), source)
                    })
                    .collect();
                (end.name.clone(), segments)
            })
            .collect();
        let nets = resolution
            .nets
            .iter()
            .map(|net| (net.name.clone(), net.width))
            .collect();
        Connections { ends, nets }
    }
}

impl SliceView {
    /// A stable sort key: the end, the range and the typed selection.
    fn order(&self) -> (&EndName, usize, usize, Option<String>) {
        let select = self.select.as_deref().map(path_string);
        (&self.end, self.msb, self.lsb, select)
    }

    fn new(slice: &PortSlice) -> Self {
        let end = match &slice.port {
            Port::ModDef { name, .. } => EndName::Def(name.clone()),
            Port::ModInst {
                hierarchy,
                port_name,
            } => EndName::Inst(
                hierarchy
                    .iter()
                    .map(|elem| elem.inst_name.clone())
                    .collect(),
                port_name.clone(),
            ),
        };
        SliceView {
            end,
            msb: slice.msb,
            lsb: slice.lsb,
            select: slice.select.clone(),
        }
    }
}
