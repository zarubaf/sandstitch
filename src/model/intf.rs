// SPDX-License-Identifier: Apache-2.0

use parking_lot::RwLock;
use std::sync::{Arc, Weak};

use crate::model::mod_def::ModDefCore;
use crate::model::mod_inst::HierPathElem;
use crate::{ConvertibleToPortSliceVec, MetadataKey, MetadataValue, ModInst, PortSlice};

mod connect;
mod copy;
pub(crate) mod crossover;
mod debug;
mod export;
mod feedthrough;
pub(crate) mod members;
mod subdivide;
mod tieoff;
mod width;

pub(crate) use members::IntfMember;

/// Represents an interface on a module definition or module instance.
/// Interfaces are used to connect modules together by function name.
#[derive(Clone)]
pub enum Intf {
    ModDef {
        name: String,
        mod_def_core: Weak<RwLock<ModDefCore>>,
    },
    ModInst {
        intf_name: String,
        hierarchy: Vec<HierPathElem>,
    },
}

impl Intf {
    /// Returns the name this interface has in its (parent) module definition.
    pub fn name(&self) -> &str {
        match self {
            Intf::ModDef { name, .. } => name,
            Intf::ModInst { intf_name, .. } => intf_name,
        }
    }

    pub(crate) fn get_mod_def_core(&self) -> Arc<RwLock<ModDefCore>> {
        match self {
            Intf::ModDef { mod_def_core, .. } => mod_def_core.upgrade().unwrap(),
            Intf::ModInst { hierarchy, .. } => hierarchy
                .last()
                .expect("Intf::ModInst hierarchy cannot be empty")
                .mod_def_core
                .upgrade()
                .expect("Containing ModDefCore has been dropped"),
        }
    }

    /// Returns the port slice for the given function name if present, otherwise `None`.
    ///
    /// Panics with S0209 if the interface has the name in both directions;
    /// use [`Intf::members`] for those.
    #[track_caller]
    pub fn get(&self, func_name: impl AsRef<str>) -> Option<PortSlice> {
        self.unique_member(func_name.as_ref())
    }

    /// Removes and returns the port slice for the given function name if present.
    ///
    /// Panics with S0209 if the interface has the name in both directions.
    #[track_caller]
    pub fn remove(&self, func_name: impl AsRef<str>) -> Option<PortSlice> {
        let slice = self.get(func_name.as_ref())?;
        let core = match self {
            Intf::ModDef { .. } => self.get_mod_def_core(),
            Intf::ModInst { hierarchy, .. } => ModInst {
                hierarchy: hierarchy.clone(),
            }
            .mod_def_core_of_instance(),
        };
        let mut binding = core.write();
        let members = binding.interfaces.get_mut(self.name())?;
        let i = members.iter().position(|m| m.name == func_name.as_ref())?;
        members.remove(i);
        Some(slice)
    }

    /// Returns an iterator over the interface functions and their port slices.
    #[track_caller]
    pub fn iter(&self) -> indexmap::map::IntoIter<String, PortSlice> {
        self.get_port_slices().into_iter()
    }

    /// Returns an iterator over the interface function names.
    #[track_caller]
    pub fn keys(&self) -> indexmap::map::IntoKeys<String, PortSlice> {
        self.get_port_slices().into_keys()
    }

    /// Returns an iterator over the interface port slices.
    #[track_caller]
    pub fn values(&self) -> indexmap::map::IntoValues<String, PortSlice> {
        self.get_port_slices().into_values()
    }

    pub(crate) fn get_intf_name(&self) -> String {
        match self {
            Intf::ModDef { name, .. } => name.clone(),
            Intf::ModInst { intf_name, .. } => intf_name.clone(),
        }
    }

    pub fn set_metadata(
        &self,
        key: impl Into<MetadataKey>,
        value: impl Into<MetadataValue>,
    ) -> Self {
        let key = key.into();
        let value = value.into();
        match self {
            Intf::ModDef { .. } => {
                let core_rc = self.get_mod_def_core();
                let mut core = core_rc.write();
                core.mod_def_intf_metadata
                    .entry(self.get_intf_name())
                    .or_default()
                    .insert(key, value);
            }
            Intf::ModInst { hierarchy, .. } => {
                let inst_name = hierarchy
                    .last()
                    .expect("Intf::ModInst hierarchy cannot be empty")
                    .inst_name
                    .to_string();
                let core_rc = self.get_mod_def_core();
                let mut core = core_rc.write();
                core.mod_inst_intf_metadata
                    .entry(inst_name)
                    .or_default()
                    .entry(self.get_intf_name())
                    .or_default()
                    .insert(key, value);
            }
        }
        self.clone()
    }

    pub fn get_metadata(&self, key: impl AsRef<str>) -> Option<MetadataValue> {
        match self {
            Intf::ModDef { .. } => {
                let core_rc = self.get_mod_def_core();
                let core = core_rc.read();
                core.mod_def_intf_metadata
                    .get(&self.get_intf_name())
                    .and_then(|metadata| metadata.get(key.as_ref()).cloned())
            }
            Intf::ModInst { hierarchy, .. } => {
                let inst_name = hierarchy
                    .last()
                    .expect("Intf::ModInst hierarchy cannot be empty")
                    .inst_name
                    .as_str();
                let core_rc = self.get_mod_def_core();
                let core = core_rc.read();
                core.mod_inst_intf_metadata
                    .get(inst_name)
                    .and_then(|intfs| intfs.get(&self.get_intf_name()))
                    .and_then(|metadata| metadata.get(key.as_ref()).cloned())
            }
        }
    }

    pub fn clear_metadata(&self, key: impl AsRef<str>) -> Self {
        match self {
            Intf::ModDef { .. } => {
                let core_rc = self.get_mod_def_core();
                let mut core = core_rc.write();
                if let Some(metadata) = core.mod_def_intf_metadata.get_mut(&self.get_intf_name()) {
                    metadata.remove(key.as_ref());
                    if metadata.is_empty() {
                        core.mod_def_intf_metadata.remove(&self.get_intf_name());
                    }
                }
            }
            Intf::ModInst { hierarchy, .. } => {
                let inst_name = hierarchy
                    .last()
                    .expect("Intf::ModInst hierarchy cannot be empty")
                    .inst_name
                    .as_str();
                let core_rc = self.get_mod_def_core();
                let mut core = core_rc.write();
                if let Some(intfs) = core.mod_inst_intf_metadata.get_mut(inst_name) {
                    let intf_name = self.get_intf_name();
                    if let Some(metadata) = intfs.get_mut(&intf_name) {
                        metadata.remove(key.as_ref());
                        if metadata.is_empty() {
                            intfs.remove(&intf_name);
                        }
                    }
                    if intfs.is_empty() {
                        core.mod_inst_intf_metadata.remove(inst_name);
                    }
                }
            }
        }
        self.clone()
    }
}

impl ConvertibleToPortSliceVec for Intf {
    #[track_caller]
    fn to_port_slice_vec(&self) -> Vec<PortSlice> {
        self.get_port_slices().into_values().collect()
    }
}
