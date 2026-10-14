// SPDX-License-Identifier: Apache-2.0

use crate::IntfKind;
use parking_lot::RwLock;
use std::hash::{Hash, Hasher};

use std::sync::{Arc, Weak};

use num_bigint::BigInt;

use crate::{
    ConvertibleToModDef, Intf, MetadataKey, MetadataValue, ModDef, ModDefCore, Port, PortSlice,
};

/// Represents an instance of a module definition, like `<mod_def_name>
/// <mod_inst_name> ( ... );` in Verilog.
#[derive(Clone, Debug)]
pub(crate) struct HierPathElem {
    pub(crate) mod_def_core: Weak<RwLock<ModDefCore>>,
    pub(crate) inst_name: String,
}

impl PartialEq for HierPathElem {
    fn eq(&self, other: &Self) -> bool {
        match (self.mod_def_core.upgrade(), other.mod_def_core.upgrade()) {
            (Some(a_rc), Some(b_rc)) => {
                Arc::ptr_eq(&a_rc, &b_rc) && (self.inst_name == other.inst_name)
            }
            _ => false,
        }
    }
}

impl Hash for HierPathElem {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mod_def_core.upgrade().unwrap().read().name.hash(state);
        self.inst_name.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct ModInst {
    pub(crate) hierarchy: Vec<HierPathElem>,
}

impl ModInst {
    pub(crate) fn mod_def_core_where_instantiated(&self) -> Arc<RwLock<ModDefCore>> {
        self.hierarchy
            .last()
            .expect("ModInst hierarchy cannot be empty")
            .mod_def_core
            .upgrade()
            .expect("Containing ModDefCore has been dropped")
    }

    pub(crate) fn mod_def_core_of_instance(&self) -> Arc<RwLock<ModDefCore>> {
        let inst_name = self.name().to_string();
        self.mod_def_core_where_instantiated()
            .read()
            .instances
            .get(&inst_name)
            .unwrap_or_else(|| panic!("Instance named {} not found", inst_name))
            .clone()
    }

    /// Returns the name of this module instance.
    pub fn name(&self) -> &str {
        &self
            .hierarchy
            .last()
            .expect("ModInst hierarchy cannot be empty")
            .inst_name
    }

    pub fn set_metadata(
        &self,
        key: impl Into<MetadataKey>,
        value: impl Into<MetadataValue>,
    ) -> Self {
        let inst_name = self.name().to_string();
        let core_rc = self.mod_def_core_where_instantiated();
        let mut core = core_rc.write();
        core.mod_inst_metadata
            .entry(inst_name)
            .or_default()
            .insert(key.into(), value.into());
        self.clone()
    }

    pub fn get_metadata(&self, key: impl AsRef<str>) -> Option<MetadataValue> {
        let inst_name = self.name().to_string();
        let core_rc = self.mod_def_core_where_instantiated();
        let core = core_rc.read();
        core.mod_inst_metadata
            .get(&inst_name)
            .and_then(|metadata| metadata.get(key.as_ref()).cloned())
    }

    pub fn clear_metadata(&self, key: impl AsRef<str>) -> Self {
        let inst_name = self.name().to_string();
        let core_rc = self.mod_def_core_where_instantiated();
        let mut core = core_rc.write();
        if let Some(metadata) = core.mod_inst_metadata.get_mut(&inst_name) {
            metadata.remove(key.as_ref());
            if metadata.is_empty() {
                core.mod_inst_metadata.remove(&inst_name);
            }
        }
        self.clone()
    }

    /// Returns `true` if this module instance has an interface with the given
    /// name.
    pub fn has_intf(&self, name: impl AsRef<str>) -> bool {
        ModDef {
            core: self.mod_def_core_of_instance(),
        }
        .has_intf(name)
    }

    /// Returns `true` if this module instance has a port with the given name.
    pub fn has_port(&self, name: impl AsRef<str>) -> bool {
        ModDef {
            core: self.mod_def_core_of_instance(),
        }
        .has_port(name)
    }

    /// First, get the module definition for this instance. Then, return the
    /// module instance with the given name in that module defintion.
    pub fn get_instance(&self, name: impl AsRef<str>) -> ModInst {
        let child = self.get_mod_def().get_instance(name.as_ref());
        let mut combined = self.hierarchy.clone();
        for frame in &child.hierarchy {
            combined.push(frame.clone());
        }
        ModInst {
            hierarchy: combined,
        }
    }

    /// Returns a vector of all module instances within this module instance.
    pub fn get_instances(&self) -> Vec<ModInst> {
        self.get_mod_def()
            .get_instances()
            .into_iter()
            .map(|child| {
                let mut combined = self.hierarchy.clone();
                combined.extend(child.hierarchy);
                ModInst {
                    hierarchy: combined,
                }
            })
            .collect()
    }

    /// Marks all ports on this instance as unused or ties them off to the given
    /// value.
    #[track_caller]
    pub fn unused_and_tieoff<T: Into<BigInt> + Clone>(&self, value: T) {
        let value_as_big_int = value.into();

        for port in self.get_ports(None) {
            port.unused_or_tieoff(value_as_big_int.clone());
        }
    }

    /// Returns the port on this instance with the given name. Panics if no such
    /// port exists.
    pub fn get_port(&self, name: impl AsRef<str>) -> Port {
        ModDef {
            core: self.mod_def_core_of_instance(),
        }
        .get_port(name)
        .assign_to_inst(self)
    }

    /// Returns a slice of the port on this instance with the given name, from
    /// `msb` down to `lsb`, inclusive. Panics if no such port exists.
    pub fn get_port_slice(&self, name: impl AsRef<str>, msb: usize, lsb: usize) -> PortSlice {
        self.get_port(name).slice(msb, lsb)
    }

    /// Returns a vector of ports on this instance with the given prefix, or all
    /// ports if `prefix` is `None`.
    pub fn get_ports(&self, prefix: Option<&str>) -> Vec<Port> {
        let result = ModDef {
            core: self.mod_def_core_of_instance(),
        }
        .get_ports(prefix);
        result
            .into_iter()
            .map(|port| port.assign_to_inst(self))
            .collect()
    }

    /// Returns the interface on this instance with the given name. Panics if no
    /// such interface exists.
    pub fn get_intf(&self, name: impl AsRef<str>) -> Intf {
        let mod_def_core = self.mod_def_core_where_instantiated();
        let instances = &mod_def_core.read().instances;

        let inst_core = match instances.get(self.name()) {
            Some(inst_core) => inst_core.clone(),
            None => panic!(
                "Interface '{}' does not exist on module definition '{}'",
                name.as_ref(),
                mod_def_core.read().name
            ),
        };

        let inst_core_borrowed = inst_core.read();

        if inst_core_borrowed.interfaces.contains_key(name.as_ref()) {
            Intf {
                kind: IntfKind::ModInst {
                    intf_name: name.as_ref().to_string(),
                    hierarchy: self.hierarchy.clone(),
                },
            }
        } else {
            panic!(
                "Interface '{}' does not exist in instance '{}'",
                name.as_ref(),
                self.debug_string()
            );
        }
    }

    /// Returns a vector of all interfaces on this module instance with the
    /// given prefix. If `prefix` is `None`, returns all interfaces.
    pub fn get_intfs(&self, prefix: Option<&str>) -> Vec<Intf> {
        self.get_mod_def()
            .get_intfs(prefix)
            .into_iter()
            .map(|intf| match intf.kind {
                IntfKind::ModDef { name, .. } => Intf {
                    kind: IntfKind::ModInst {
                        intf_name: name,
                        hierarchy: self.hierarchy.clone(),
                    },
                },
                IntfKind::ModInst { .. } => intf,
            })
            .collect()
    }

    /// Returns the parent instance of this instance.
    pub fn get_parent(&self) -> Option<ModInst> {
        if self.hierarchy.len() <= 1 {
            None
        } else {
            Some(ModInst {
                hierarchy: self.hierarchy[..self.hierarchy.len() - 1].to_vec(),
            })
        }
    }

    /// Returns the ModDef that this instance is instantiated in.
    pub fn get_mod_def_where_instantiated(&self) -> ModDef {
        ModDef {
            core: self.mod_def_core_where_instantiated(),
        }
    }

    /// Returns the ModDef that this is an instance of.
    pub fn get_mod_def(&self) -> ModDef {
        ModDef {
            core: self.mod_def_core_of_instance(),
        }
    }

    pub(crate) fn debug_string(&self) -> String {
        let mut parts = Vec::new();
        if let Some(frame) = self.hierarchy.first() {
            parts.push(frame.mod_def_core.upgrade().unwrap().read().name.clone());
        }
        for frame in &self.hierarchy {
            parts.push(frame.inst_name.clone());
        }
        parts.join(".")
    }
}

impl ConvertibleToModDef for ModInst {
    fn to_mod_def(&self) -> ModDef {
        self.get_mod_def()
    }
    fn get_port(&self, name: impl AsRef<str>) -> Port {
        self.get_port(name)
    }
    fn get_intf(&self, name: impl AsRef<str>) -> Intf {
        self.get_intf(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_inst_hierarchy_extends_with_get_instance() {
        let mod_c = ModDef::new("C");

        let mod_b = ModDef::new("B");
        mod_b.instantiate(&mod_c, Some("c_inst"), None);

        let mod_a = ModDef::new("A");
        mod_a.instantiate(&mod_b, Some("b_inst"), None);

        let b_inst = mod_a.get_instance("b_inst");
        assert_eq!(b_inst.debug_string(), "A.b_inst");
        assert_eq!(b_inst.hierarchy.len(), 1);
        assert_eq!(b_inst.hierarchy[0].inst_name, "b_inst");
        assert_eq!(
            b_inst.hierarchy[0]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "A"
        );

        let c_from_b = b_inst.get_instance("c_inst");
        assert_eq!(c_from_b.debug_string(), "A.b_inst.c_inst");
        assert_eq!(c_from_b.name(), "c_inst");
        assert_eq!(c_from_b.get_mod_def().get_name(), "C");
        assert_eq!(c_from_b.hierarchy.len(), 2);
        assert_eq!(c_from_b.hierarchy[0].inst_name, "b_inst");
        assert_eq!(c_from_b.hierarchy[1].inst_name, "c_inst");
        assert_eq!(
            c_from_b.hierarchy[0]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "A"
        );
        assert_eq!(
            c_from_b.hierarchy[1]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "B"
        );

        let c_direct = mod_b.get_instance("c_inst");
        assert_eq!(c_direct.debug_string(), "B.c_inst");
        assert_eq!(c_direct.hierarchy.len(), 1);
        assert_eq!(c_direct.hierarchy[0].inst_name, "c_inst");
        assert_eq!(
            c_direct.hierarchy[0]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "B"
        );
    }

    #[test]
    fn mod_inst_hierarchy_extends_with_get_instances() {
        let leaf = ModDef::new("Leaf");

        let mid = leaf.wrap(Some("Mid"), Some("leaf_inst"));
        let top = mid.wrap(Some("Top"), Some("mid_inst"));

        let mid_inst = top.get_instance("mid_inst");
        let children = mid_inst.get_instances();
        assert_eq!(children.len(), 1);

        let child = &children[0];
        assert_eq!(child.debug_string(), "Top.mid_inst.leaf_inst");
        assert_eq!(child.name(), "leaf_inst");
        assert_eq!(child.get_mod_def().get_name(), "Leaf");
        assert_eq!(child.hierarchy.len(), 2);
        assert_eq!(child.hierarchy[0].inst_name, "mid_inst");
        assert_eq!(child.hierarchy[1].inst_name, "leaf_inst");
        assert_eq!(
            child.hierarchy[0]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "Top"
        );
        assert_eq!(
            child.hierarchy[1]
                .mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .name,
            "Mid"
        );
    }

    #[test]
    fn mod_inst_debug_string_handles_deep_hierarchy() {
        let mod_d = ModDef::new("D");

        let mod_c = ModDef::new("C");
        mod_c.instantiate(&mod_d, Some("d_inst"), None);

        let mod_b = ModDef::new("B");
        mod_b.instantiate(&mod_c, Some("c_inst"), None);

        let mod_a = ModDef::new("A");
        mod_a.instantiate(&mod_b, Some("b_inst"), None);

        let d_from_a = mod_a
            .get_instance("b_inst")
            .get_instance("c_inst")
            .get_instance("d_inst");

        assert_eq!(d_from_a.debug_string(), "A.b_inst.c_inst.d_inst");
        assert_eq!(d_from_a.hierarchy.len(), 3);
        assert_eq!(d_from_a.hierarchy[0].inst_name, "b_inst");
        assert_eq!(d_from_a.hierarchy[1].inst_name, "c_inst");
        assert_eq!(d_from_a.hierarchy[2].inst_name, "d_inst");
    }
}
