// SPDX-License-Identifier: Apache-2.0

use crate::ModDef;

impl ModDef {
    /// Report all instances of this module, descending hierarchically. The
    /// returned vector contains tuples of (module definition name, module
    /// instance name).
    pub fn report_all_instances(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        report_all_instances_helper(self, &self.get_name(), &mut result);
        result
    }
}

fn report_all_instances_helper(
    parent: &ModDef,
    parent_path: &str,
    result: &mut Vec<(String, String)>,
) {
    for child_inst in parent.get_instances().iter() {
        let full_inst_name = format!("{}.{}", parent_path, child_inst.name());
        let child_mod_def = child_inst.get_mod_def();
        report_all_instances_helper(&child_mod_def, &full_inst_name, result);
        result.push((child_mod_def.get_name().to_string(), full_inst_name));
    }
}
