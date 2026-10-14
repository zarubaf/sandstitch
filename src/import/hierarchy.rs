// SPDX-License-Identifier: Apache-2.0

use crate::{ModDef, Span};

pub(crate) fn populate_hierarchy(parent: &ModDef, inst: &slang_rs::Instance) {
    for child in inst.contents.iter() {
        let child_def_name = &child.borrow().def_name;
        let mut child_inst_name = child.borrow().inst_name.clone();
        let mut child_hier_prefix = child.borrow().hier_prefix.clone();
        if child_hier_prefix.starts_with(".") {
            child_hier_prefix = child_hier_prefix[1..].to_string();
        }
        if child_hier_prefix.ends_with(".") {
            child_hier_prefix = child_hier_prefix[..child_hier_prefix.len() - 1].to_string();
        }
        if !child_hier_prefix.is_empty() {
            child_inst_name = format!("{}.{}", child_hier_prefix, child.borrow().inst_name);
        }
        // The definition comes from the imported source, not from a Rust
        // call, so it has no span.
        let child_mod_def = ModDef::new(child_def_name);
        child_mod_def.core.write().span = Span::None;
        parent.instantiate(&child_mod_def, Some(&child_inst_name), None);
        populate_hierarchy(&child_mod_def, &child.borrow());
    }
}

#[cfg(test)]
mod tests {
    use crate::{ModDef, ParserConfig, Span};

    #[test]
    fn imported_hierarchy_definitions_have_no_span() {
        let source =
            slang_rs::str2tmpfile("module C; endmodule\nmodule A; C c0(); endmodule\n").unwrap();
        let cfg = ParserConfig {
            sources: &[source.path().to_str().unwrap()],
            include_hierarchy: true,
            ..Default::default()
        };
        let top = ModDef::from_verilog_with_config("A", &cfg);
        let child = top.get_instance("c0").get_mod_def();
        assert_eq!(child.core.read().span, Span::None);
    }
}
