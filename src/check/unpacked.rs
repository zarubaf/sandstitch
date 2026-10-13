// SPDX-License-Identifier: Apache-2.0

//! Connections of unpacked structs and unions (S0206).
//!
//! An unpacked aggregate has no bit layout, so emitted RTL can only connect
//! it whole, through a net declared with its type. Emission declares such a
//! net only for a type named with its package
//! ([`declarable`](crate::emit::declarable)) whose uses are all whole and of
//! equivalent types; anything else would be a flat vector, a bit slice or a
//! literal, which SystemVerilog does not assign to an unpacked aggregate.
//! Unpacked types are also nominal: an end of another named type with the
//! same layout does not connect either.

use std::collections::HashSet;

use crate::diagnostic::codes;
use crate::emit::declarable;
use crate::model::connection::connected_item::ConnectedItem;
use crate::model::connection::port_slice::PortSliceConnection;
use crate::{Diagnostic, Label, Port, PortSlice, PortType, Severity};

pub(super) fn is_unpacked(ty: &PortType) -> bool {
    matches!(
        ty,
        PortType::Struct {
            is_packed: false,
            ..
        } | PortType::Union {
            is_packed: false,
            ..
        }
    )
}

fn is_whole(slice: &PortSlice) -> bool {
    slice.lsb == 0 && slice.msb + 1 == slice.port.io().width()
}

/// Why `connection`, whose `this` end has unpacked type `ty`, cannot be
/// emitted; `None` if it can.
fn problem(connection: &PortSliceConnection, ty: &PortType) -> Option<String> {
    let this = &connection.this;
    if !is_whole(this) {
        return Some(format!(
            "only bits [{}:{}] of it are connected",
            this.msb, this.lsb
        ));
    }
    if declarable(ty).is_none() {
        return Some(format!(
            "`{ty}` is not named with its package, so no net can be declared with it"
        ));
    }
    let other = match &connection.other {
        ConnectedItem::Unused(_) => return None,
        ConnectedItem::Tieoff(_) => return Some("it is tied off".into()),
        ConnectedItem::Wire(wire) => {
            return Some(format!("it is connected to wire `{}`", wire.name));
        }
        ConnectedItem::PortSlice(other) => other,
    };
    let name = other.port.debug_string();
    if !is_whole(other) {
        return Some(format!(
            "it is connected to bits [{}:{}] of `{name}`",
            other.msb, other.lsb
        ));
    }
    match other.port.port_type() {
        None => Some(format!("it is connected to `{name}`, which has no type")),
        Some(other_ty) if ty.equivalent(&other_ty).is_err() => {
            Some(format!("it is connected to `{name}` of type `{other_ty}`"))
        }
        Some(other_ty) if declarable(&other_ty).is_none() => Some(format!(
            "it is connected to `{name}` of type `{other_ty}`, which is not named with its \
             package"
        )),
        // Unpacked types are nominal: equal layouts of different types do
        // not connect. A typedef alias is reported under its own name, so an
        // alias connected to the type it names is reported too.
        Some(other_ty) if declarable(&other_ty) != declarable(ty) => Some(format!(
            "it is connected to `{name}` of type `{other_ty}`, a different type with the same \
             layout; unpacked types connect only to the same named type"
        )),
        Some(_) => None,
    }
}

/// S0206 for each port with an unpacked struct or union type that one of
/// `connections` (those of one module) would connect other than whole,
/// through a net of its type. One diagnostic per port.
///
/// Instance ports keep their declared type, so each is checked. A port of
/// the module itself is declared by emission, flat if need be. In an
/// instantiated definition (`!is_root`) it is always checked: its instances
/// in the parent connect to it as declared by its imported type. In the root
/// (the module emitted, which nothing instantiates) a flat declaration is
/// legal on its own, so it is checked only when it is connected to an
/// instance port, whose unpacked value it then carries.
pub(super) fn check_connections(
    connections: &[PortSliceConnection],
    is_root: bool,
) -> Vec<Diagnostic> {
    let carries_instance_value: HashSet<String> = connections
        .iter()
        .filter_map(|c| match (&c.this.port, &c.other) {
            (Port::ModDef { name, .. }, ConnectedItem::PortSlice(other))
                if matches!(other.port, Port::ModInst { .. }) =>
            {
                Some(name.clone())
            }
            _ => None,
        })
        .collect();
    let mut reported = HashSet::new();
    let mut diagnostics = Vec::new();
    for connection in connections {
        let port = &connection.this.port;
        if let Port::ModDef { name, .. } = port
            && is_root
            && !carries_instance_value.contains(name)
        {
            continue;
        }
        let Some(ty) = port.port_type().filter(is_unpacked) else {
            continue;
        };
        let Some(problem) = problem(connection, &ty) else {
            continue;
        };
        let name = port.debug_string();
        if !reported.insert(name.clone()) {
            continue;
        }
        diagnostics.push(Diagnostic {
            code: codes::S0206,
            severity: Severity::Error,
            message: format!("unpacked `{ty}` port `{name}` cannot be connected as bits"),
            primary: Label {
                span: port.source_span(),
                text: format!("`{name}` is `{ty}`"),
            },
            secondary: vec![],
            notes: vec![
                problem,
                "an unpacked struct or union has no bit layout: emitted RTL can only connect \
                 it whole, through a net of its type"
                    .into(),
            ],
            helps: vec![
                "connect the whole port to a port of the same type, named with its package, \
                 or declare the type `packed`"
                    .into(),
            ],
            object: None,
        });
    }
    diagnostics
}
