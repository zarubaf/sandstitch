// SPDX-License-Identifier: Apache-2.0

//! Connection resolution: what drives each bit of each end of a definition.
//!
//! An end (a port of an instance, or a port of the definition) resolves to
//! segments, MSB first. Each segment says which net slice, constant or unused
//! marker supplies the bits of one slice of the end. Segments carry the
//! original [`PortSlice`], including its typed selection.

pub(crate) mod component;
pub(crate) mod def;
pub(crate) mod design;

use crate::PortSlice;
use crate::model::connection::connected_item::{ConnectedItem, Tieoff, Unused, Wire};

/// The slice of a net a segment reads: a port slice (with its typed
/// selection) or a range of a named wire.
#[derive(Debug)]
pub(crate) enum NetSlice {
    Port(PortSlice),
    Wire(Wire),
}

/// What supplies the bits of a segment.
#[derive(Debug)]
pub(crate) enum Source {
    Net(NetSlice),
    Constant(Tieoff),
    Unused,
}

impl From<&Source> for ConnectedItem {
    fn from(source: &Source) -> Self {
        match source {
            Source::Net(NetSlice::Port(slice)) => ConnectedItem::PortSlice(slice.clone()),
            Source::Net(NetSlice::Wire(wire)) => ConnectedItem::Wire(wire.clone()),
            Source::Constant(tieoff) => ConnectedItem::Tieoff(tieoff.clone()),
            Source::Unused => ConnectedItem::Unused(Unused::new()),
        }
    }
}

/// Bits `this` of an end come from `source` (same width).
#[derive(Debug)]
pub(crate) struct Segment {
    pub(crate) this: PortSlice,
    pub(crate) source: Source,
}

/// An end's segments, MSB first, merged where contiguous.
#[derive(Debug)]
pub(crate) struct EndResolution {
    pub(crate) name: String,
    pub(crate) segments: Vec<Segment>,
}

/// The resolved ends of one definition.
#[derive(Debug)]
pub(crate) struct Resolution {
    /// Per instance in declaration order, its ports in declaration order.
    pub(crate) instances: Vec<Vec<EndResolution>>,
    /// The definition's ports in declaration order.
    pub(crate) ports: Vec<EndResolution>,
    /// The bits joined by connections and what drives them.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the snapshot (#45)"))]
    pub(crate) components: Vec<component::Component>,
}
