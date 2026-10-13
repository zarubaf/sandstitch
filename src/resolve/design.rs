// SPDX-License-Identifier: Apache-2.0

//! Resolution of every definition that emission plans, in one pass.

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use indexmap::map::Entry;
use parking_lot::RwLock;

use super::Resolution;
use super::def::resolve_def;
use crate::{ModDef, ModDefCore, Usage};

/// The resolutions of a design's definitions, keyed by core identity.
pub(crate) struct DesignResolution {
    resolutions: HashMap<*const RwLock<ModDefCore>, Resolution>,
}

impl DesignResolution {
    /// The resolution of `core`, if emission plans it as a definition.
    pub(crate) fn get(&self, core: &Arc<RwLock<ModDefCore>>) -> Option<&Resolution> {
        self.resolutions.get(&Arc::as_ptr(core))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.resolutions.len()
    }
}

/// Resolves, children before parents and each once, every definition
/// emission plans: reached from `top` as emission descends, with usage
/// `EmitDefinitionAndDescend`.
pub(crate) fn resolve_design(top: &ModDef) -> DesignResolution {
    let mut design = DesignResolution {
        resolutions: HashMap::new(),
    };
    let mut registered = IndexMap::new();
    visit(&top.core, &mut registered, &mut design);
    design
}

fn visit(
    core: &Arc<RwLock<ModDefCore>>,
    registered: &mut IndexMap<String, Arc<RwLock<ModDefCore>>>,
    design: &mut DesignResolution,
) {
    // Resolving locks cores, so no guard is held past this block.
    let (usage, children) = {
        let guard = core.read();
        if guard.usage == Usage::EmitNothingAndStop || guard.usage == Usage::EmitDefinitionAndStop {
            return;
        }
        match registered.entry(guard.name.clone()) {
            Entry::Occupied(entry) => {
                if !Arc::ptr_eq(entry.get(), core) {
                    panic!("Two distinct modules with the same name: {}", guard.name);
                }
                return;
            }
            Entry::Vacant(entry) => {
                entry.insert(core.clone());
            }
        }
        let children: Vec<_> = if guard.usage == Usage::EmitDefinitionAndDescend {
            guard.instances.values().cloned().collect()
        } else {
            Vec::new()
        };
        (guard.usage.clone(), children)
    };

    for child in &children {
        visit(child, registered, design);
    }

    if usage == Usage::EmitDefinitionAndDescend {
        let previous = design
            .resolutions
            .insert(Arc::as_ptr(core), resolve_def(core));
        debug_assert!(previous.is_none(), "definition resolved twice");
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_design;
    use crate::{IO, ModDef, Usage};

    fn opaque(name: &str, port: &str, io: IO) -> ModDef {
        let leaf = ModDef::new(name);
        leaf.add_port(port, io);
        leaf.set_usage(Usage::EmitNothingAndStop);
        leaf
    }

    #[test]
    fn resolves_each_planned_definition_once() {
        let leaf = opaque("leaf", "i", IO::Input(1));
        let child = ModDef::new("child");
        child.add_port("i", IO::Input(1));
        let c_leaf = child.instantiate(&leaf, Some("l"), None);
        child.get_port("i").connect(&c_leaf.get_port("i"));

        let top = ModDef::new("top");
        top.add_port("i", IO::Input(1));
        let c0 = top.instantiate(&child, Some("c0"), None);
        let c1 = top.instantiate(&child, Some("c1"), None);
        top.get_port("i").connect(&c0.get_port("i"));
        top.get_port("i").connect(&c1.get_port("i"));
        let l = top.instantiate(&leaf, Some("l"), None);
        top.get_port("i").connect(&l.get_port("i"));

        let design = resolve_design(&top);
        assert!(design.get(&top.core).is_some());
        assert!(design.get(&child.core).is_some());
        assert!(design.get(&leaf.core).is_none());
        assert_eq!(design.len(), 2);
    }

    #[test]
    #[should_panic(expected = "child.l.i is unconnected")]
    fn children_resolve_before_parents() {
        let leaf = opaque("leaf", "i", IO::Input(1));
        let child = ModDef::new("child");
        child.instantiate(&leaf, Some("l"), None);
        let top = ModDef::new("top");
        top.add_port("o", IO::Output(1));
        top.instantiate(&child, Some("c"), None);
        resolve_design(&top);
    }

    #[test]
    #[should_panic(expected = "Two distinct modules with the same name: leaf")]
    fn same_name_check_keeps_its_place() {
        let first = ModDef::new("leaf");
        first.add_port("i", IO::Input(1));
        first.set_usage(Usage::EmitDefinitionAndDescend);
        let second = ModDef::new("leaf");
        second.add_port("i", IO::Input(1));
        second.set_usage(Usage::EmitDefinitionAndDescend);
        let top = ModDef::new("top");
        top.add_port("i", IO::Input(1));
        let a = top.instantiate(&first, Some("a"), None);
        let b = top.instantiate(&second, Some("b"), None);
        top.get_port("i").connect(&a.get_port("i"));
        top.get_port("i").connect(&b.get_port("i"));
        first.get_port("i").unused();
        resolve_design(&top);
    }

    #[test]
    fn resolve_design_terminates_on_cycles() {
        let a = ModDef::new("a");
        a.add_port("x", IO::Output(8));
        let b = a.wrap(Some("b"), Some("a_inst"));
        let b_inst = a.instantiate(&b, Some("b_inst"), None);
        b_inst.get_port("x").connect(&a.get_port("x"));
        resolve_design(&a);
    }
}
