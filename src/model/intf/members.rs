// SPDX-License-Identifier: Apache-2.0

//! Interface members: a name and a reference to a port slice, with its
//! typed selection, so that slices built with `field` and `index` stay typed.

use std::sync::Arc;

use crate::diagnostic::codes;
use crate::model::port::PortDirectionality;
use crate::model::port_slice::path_string;
use crate::{
    Diagnostic, IO, Intf, Label, ModDef, ModInst, Port, PortSlice, Select, Severity, Span,
};

/// A stored interface member. `port` names a port of the module definition
/// the interface is defined on.
#[derive(Clone, Debug)]
pub(crate) struct IntfMember {
    pub(crate) name: String,
    pub(crate) port: String,
    pub(crate) msb: usize,
    pub(crate) lsb: usize,
    pub(crate) select: MemberSelect,
}

/// How a member's slice is formed when it is read.
#[derive(Clone, Debug)]
pub(crate) enum MemberSelect {
    /// From a `(port, msb, lsb)` mapping (`def_intf`), resolved when read
    /// as upstream, so the port may be added after the interface: the
    /// whole port is typed, any other range is a plain bit slice.
    Range,
    /// The selection of the slice the member was defined with.
    Slice(Option<Arc<[Select]>>),
}

impl IntfMember {
    pub(crate) fn new(name: impl Into<String>, slice: &PortSlice) -> Self {
        IntfMember {
            name: name.into(),
            port: slice.port.name().to_string(),
            msb: slice.msb,
            lsb: slice.lsb,
            select: MemberSelect::Slice(slice.select.clone()),
        }
    }

    /// A member from a `(port, msb, lsb)` mapping; the port is not looked up.
    pub(crate) fn range(name: impl Into<String>, port: &str, msb: usize, lsb: usize) -> Self {
        IntfMember {
            name: name.into(),
            port: port.to_string(),
            msb,
            lsb,
            select: MemberSelect::Range,
        }
    }

    /// The member as a slice of `port`, the member's port seen from the
    /// definition or from an instance.
    fn slice_of(&self, port: Port) -> PortSlice {
        match &self.select {
            MemberSelect::Range => port.slice_or_whole(self.msb, self.lsb),
            MemberSelect::Slice(select) => PortSlice {
                port,
                msb: self.msb,
                lsb: self.lsb,
                select: select.clone(),
            },
        }
    }
}

/// `node.req_o[1]` for a typed selection, the bit range otherwise.
pub(crate) fn describe(slice: &PortSlice) -> String {
    match &slice.select {
        Some(path) => format!("{}{}", slice.port.debug_string(), path_string(path)),
        None => slice.debug_string(),
    }
}

/// `input`, `output` or `inout`: the direction of the member's port.
pub(crate) fn direction(slice: &PortSlice) -> &'static str {
    match slice.port.io() {
        IO::Input(_) => "input",
        IO::Output(_) => "output",
        IO::InOut(_) => "inout",
    }
}

/// Whether one member drives the other, seen from the module that connects
/// them: a driver with a receiver, or two inouts.
pub(crate) fn pairs_with(a: &PortSlice, b: &PortSlice) -> bool {
    matches!(
        (a.get_directionality(), b.get_directionality()),
        (PortDirectionality::Driver, PortDirectionality::Receiver)
            | (PortDirectionality::Receiver, PortDirectionality::Driver)
            | (PortDirectionality::InOut, PortDirectionality::InOut)
    )
}

impl Intf {
    /// Returns the members of this interface in definition order, as
    /// `(name, slice)` pairs. A name appears twice when the interface has it
    /// in both directions. Slices keep their typed selection.
    pub fn members(&self) -> Vec<(String, PortSlice)> {
        match self {
            Intf::ModDef {
                mod_def_core, name, ..
            } => {
                let core = mod_def_core.upgrade().unwrap();
                let mod_def = ModDef { core: core.clone() };
                let stored = core.read().interfaces.get(name).unwrap().clone();
                stored
                    .iter()
                    .map(|m| (m.name.clone(), m.slice_of(mod_def.get_port(&m.port))))
                    .collect()
            }
            Intf::ModInst {
                intf_name,
                hierarchy,
                ..
            } => {
                let inst = ModInst {
                    hierarchy: hierarchy.clone(),
                };
                let stored = inst
                    .mod_def_core_of_instance()
                    .read()
                    .interfaces
                    .get(intf_name)
                    .unwrap()
                    .clone();
                stored
                    .iter()
                    .map(|m| (m.name.clone(), m.slice_of(inst.get_port(&m.port))))
                    .collect()
            }
        }
    }

    /// The members by name, for operations that pair members by name only.
    /// Panics with S0209 if a name is present in both directions.
    #[track_caller]
    pub(crate) fn get_port_slices(&self) -> indexmap::IndexMap<String, PortSlice> {
        let members = self.members();
        if let Some(name) = repeated_name(&members) {
            self.panic_s0209(&name, "this operation pairs members by name only");
        }
        members.into_iter().collect()
    }

    /// The only member named `name`; S0209 if it is present in both
    /// directions.
    #[track_caller]
    pub(crate) fn unique_member(&self, name: &str) -> Option<PortSlice> {
        let mut found = self.members().into_iter().filter(|(n, _)| n == name);
        let first = found.next().map(|(_, slice)| slice);
        if found.next().is_some() {
            self.panic_s0209(
                name,
                "a name present in both directions does not identify one member",
            );
        }
        first
    }

    #[track_caller]
    fn panic_s0209(&self, name: &str, why: &str) -> ! {
        Diagnostic {
            code: codes::S0209,
            severity: Severity::Error,
            message: format!(
                "interface `{}` has member `{name}` in both directions",
                self.debug_string()
            ),
            primary: Label {
                span: Span::caller(),
                text: "used here".into(),
            },
            secondary: vec![],
            notes: vec![why.into()],
            helps: vec![
                "use `connect`, `export_with_prefix`, `tieoff`, `unused` or `members`, which \
                 tell the directions apart"
                    .into(),
            ],
            object: None,
        }
        .panic()
    }
}

/// A name that appears more than once.
fn repeated_name(members: &[(String, PortSlice)]) -> Option<String> {
    members
        .iter()
        .enumerate()
        .find(|(i, (name, _))| members[..*i].iter().any(|(n, _)| n == name))
        .map(|(_, (name, _))| name.clone())
}
