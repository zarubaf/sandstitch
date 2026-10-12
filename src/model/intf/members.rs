// SPDX-License-Identifier: Apache-2.0

//! Interface members: a name and a reference to a port slice, with its
//! typed selection, so that slices built with `field` and `index` stay typed.

use std::sync::Arc;

use crate::{Intf, ModDef, ModInst, Port, PortSlice, Select};

/// A stored interface member. `port` names a port of the module definition
/// the interface is defined on.
#[derive(Clone, Debug)]
pub(crate) struct IntfMember {
    pub(crate) name: String,
    pub(crate) port: String,
    pub(crate) msb: usize,
    pub(crate) lsb: usize,
    pub(crate) select: Option<Arc<[Select]>>,
}

impl IntfMember {
    pub(crate) fn new(name: impl Into<String>, slice: &PortSlice) -> Self {
        IntfMember {
            name: name.into(),
            port: slice.port.name().to_string(),
            msb: slice.msb,
            lsb: slice.lsb,
            select: slice.select.clone(),
        }
    }

    /// The member as a slice of `port`, the member's port seen from the
    /// definition or from an instance.
    fn slice_of(&self, port: Port) -> PortSlice {
        PortSlice {
            port,
            msb: self.msb,
            lsb: self.lsb,
            select: self.select.clone(),
        }
    }
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
}
