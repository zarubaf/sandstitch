// SPDX-License-Identifier: Apache-2.0

use crate::Intf;
use crate::IntfKind;

impl std::fmt::Debug for Intf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Interface Mapping:")?;
        for (func_name, slice) in self.members() {
            writeln!(
                f,
                "{func_name}: (port_name: {}, msb: {}, lsb: {})",
                slice.port.name(),
                slice.msb,
                slice.lsb
            )?;
        }
        Ok(())
    }
}

impl Intf {
    pub(crate) fn debug_string(&self) -> String {
        match &self.kind {
            IntfKind::ModDef { name, .. } => {
                format!("{}.{}", self.get_mod_def_core().read().name, name)
            }
            IntfKind::ModInst {
                intf_name,
                hierarchy,
                ..
            } => {
                let inst = crate::ModInst {
                    hierarchy: hierarchy.clone(),
                };
                format!("{}.{}", inst.debug_string(), intf_name)
            }
        }
    }
}
