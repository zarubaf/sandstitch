// SPDX-License-Identifier: Apache-2.0

//! A definition's identity: its module name, plus a short hash of its
//! parameter overrides when it has any. Specializations of one module have
//! distinct keys and so coexist.

use std::fmt;

use crate::ModDefCore;
use crate::import::specialize::overrides_short_hash;

/// The identity of a definition: `name`, or `name#<short hash>` when it has
/// parameter overrides.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read by the design pass (Task 2)")
)]
pub(crate) struct DefKey(String);

impl fmt::Display for DefKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The key of a definition: its specialization's short hash if it is one, a
/// hash of its overrides if it has any (a plain import with `-G`), else its
/// name.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "read by the design pass (Task 2)")
)]
pub(crate) fn def_key(core: &ModDefCore) -> DefKey {
    let specialization = core
        .verilog_import
        .as_ref()
        .and_then(|import| import.specialization.as_ref());
    if let Some(key) = specialization {
        DefKey(format!("{}#{}", core.name, key.short_hash()))
    } else if !core.parameters.is_empty() {
        DefKey(format!(
            "{}#{}",
            core.name,
            overrides_short_hash(&core.parameters)
        ))
    } else {
        DefKey(core.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parameter::{ParameterSpec, ParameterType};
    use crate::{ModDef, ParserConfig};
    use indexmap::IndexMap;
    use num_bigint::BigInt;

    fn int(value: i64, width: usize) -> ParameterSpec {
        ParameterSpec::Integer {
            value: BigInt::from(value),
            ty: ParameterType::Unsigned(width),
        }
    }

    fn overrides(items: &[(&str, ParameterSpec)]) -> IndexMap<String, ParameterSpec> {
        items
            .iter()
            .map(|(name, spec)| (name.to_string(), spec.clone()))
            .collect()
    }

    #[test]
    fn generated_definition_key_is_its_name() {
        assert_eq!(
            def_key(&ModDef::new("leaf").core.read()),
            DefKey("leaf".into())
        );
    }

    #[test]
    fn specialization_key_uses_specialization_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyed.sv");
        std::fs::write(
            &path,
            "module Keyed #(parameter W = 8) (output [W-1:0] d);\nendmodule\n",
        )
        .unwrap();
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            ..Default::default()
        };
        let base = ModDef::from_verilog_with_config("Keyed", &cfg);
        let key_of = |def: &ModDef| {
            let core = def.core.read();
            let hash = core
                .verilog_import
                .as_ref()
                .and_then(|import| import.specialization.as_ref())
                .unwrap()
                .short_hash();
            (def_key(&core).to_string(), hash)
        };
        let (key4, hash4) = key_of(&base.parameterize(&[("W", 4)]));
        let (key6, hash6) = key_of(&base.parameterize(&[("W", 6)]));
        assert_eq!(hash4.len(), 12);
        assert_eq!(key4, format!("Keyed#{hash4}"));
        assert_eq!(key6, format!("Keyed#{hash6}"));
        assert_ne!(key4, key6);
    }

    #[test]
    fn override_hash_ignores_order() {
        let ab = overrides(&[("A", int(1, 32)), ("B", int(2, 32))]);
        let ba = overrides(&[("B", int(2, 32)), ("A", int(1, 32))]);
        let other = overrides(&[("A", int(1, 32)), ("B", int(3, 32))]);
        assert_eq!(overrides_short_hash(&ab), overrides_short_hash(&ba));
        assert_ne!(overrides_short_hash(&ab), overrides_short_hash(&other));
        assert_eq!(overrides_short_hash(&ab).len(), 12);
    }

    #[test]
    fn override_hash_includes_width() {
        assert_ne!(
            overrides_short_hash(&overrides(&[("A", int(1, 8))])),
            overrides_short_hash(&overrides(&[("A", int(1, 16))]))
        );
    }
}
