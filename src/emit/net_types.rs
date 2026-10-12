// SPDX-License-Identifier: Apache-2.0

//! Which nets are declared with their driver's type.
//!
//! A net (or module port) is declared with the type of the port it is named
//! after when that type is a struct or union named with its package and
//! every use of the net is *typed*: the bits it covers are a whole value of
//! the net's type (the net itself, a member or an element) whose type is
//! equivalent to a whole value covering the bits of the other end. Such a use
//! is emitted as a member select or index. If any use is not typed
//! (`connect_bits` between different layouts, a struct connected to a plain
//! vector or an untyped port, a bit range that is no member, a wire), the net
//! is declared as a flat bit vector, as in untyped designs, and every use is
//! a bit slice. A typed net is therefore never connected to an end of
//! another type.

use indexmap::IndexMap;

use crate::model::connection::connected_item::ConnectedItem;
use crate::model::port_slice::selections_covering;
use crate::{PortSlice, PortType, QualifiedName, Select};

/// The package and name of a type a declaration can name: a struct or union
/// declared in a package.
pub(super) fn declarable(ty: &PortType) -> Option<(&str, &str)> {
    match ty {
        PortType::Struct {
            name:
                Some(QualifiedName {
                    package: Some(package),
                    name,
                }),
            ..
        }
        | PortType::Union {
            name:
                Some(QualifiedName {
                    package: Some(package),
                    name,
                }),
            ..
        } => Some((package, name)),
        _ => None,
    }
}

/// What a use of a net is connected to.
pub(super) enum End<'a> {
    Slice(&'a PortSlice),
    Tieoff,
    Untyped,
}

impl<'a> End<'a> {
    pub(super) fn of(item: &'a ConnectedItem) -> Self {
        match item {
            ConnectedItem::PortSlice(slice) => End::Slice(slice),
            ConnectedItem::Tieoff(_) => End::Tieoff,
            ConnectedItem::Unused(_) | ConnectedItem::Wire(_) => End::Untyped,
        }
    }
}

/// The whole typed values whose bits are exactly those of `slice`, with the
/// selection paths reaching them: the slice's own selection first, then
/// shallower paths before deeper ones.
fn whole_values(slice: &PortSlice) -> Vec<(Vec<Select>, PortType)> {
    let Some(root) = slice.port.port_type() else {
        return Vec::new();
    };
    let mut values = Vec::new();
    if let (Some(path), Some(ty)) = (&slice.select, slice.port_type()) {
        values.push((path.to_vec(), ty));
    }
    values.extend(selections_covering(&root, slice.msb, slice.lsb));
    values
}

/// The selection path through which a typed net uses `net_slice` when
/// connected to `end`, or `None` if the use is not typed.
pub(super) fn typed_path(net_slice: &PortSlice, end: &End) -> Option<Vec<Select>> {
    let values = whole_values(net_slice);
    match end {
        End::Slice(end) => {
            let ends = whole_values(end);
            values
                .into_iter()
                .find(|(_, ty)| ends.iter().any(|(_, other)| ty.equivalent(other).is_ok()))
                .map(|(path, _)| path)
        }
        // A literal assigns to a struct, union or vector, not to an enum.
        End::Tieoff => values
            .into_iter()
            .find(|(_, ty)| !matches!(ty, PortType::Enum { .. }))
            .map(|(path, _)| path),
        End::Untyped => None,
    }
}

/// The typing decision for each net of one module, by net name.
#[derive(Default)]
pub(super) struct NetTypes {
    nets: IndexMap<String, Option<PortType>>,
}

impl NetTypes {
    /// Records a use of net `name`, which is named after the port of
    /// `net_slice`, through `net_slice`, connected to `end`.
    pub(super) fn record(&mut self, name: String, net_slice: &PortSlice, end: End) {
        let ty = self.nets.entry(name).or_insert_with(|| {
            net_slice
                .port
                .port_type()
                .filter(|ty| declarable(ty).is_some())
        });
        if ty.is_some() && typed_path(net_slice, &end).is_none() {
            *ty = None;
        }
    }

    /// The type to declare net `name` with, given the type of the port it is
    /// named after. A net without recorded uses takes the port's type.
    pub(super) fn type_of(&self, name: &str, port_type: Option<PortType>) -> Option<PortType> {
        match self.nets.get(name) {
            Some(ty) => ty.clone(),
            None => port_type.filter(|ty| declarable(ty).is_some()),
        }
    }
}
