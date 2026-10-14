// SPDX-License-Identifier: Apache-2.0

use crate::IntfKind;
use crate::PortKind;
use indexmap::IndexMap;
use regex::Regex;
use std::sync::{Arc, Weak};

use crate::diagnostic::codes;
use crate::model::intf::IntfMember;
use crate::model::intf::members::{describe, direction};
use crate::{Diagnostic, Intf, Label, ModDef, PortSlice, Severity, Span};

impl ModDef {
    /// Defines an interface with the given name. `mapping` is a map from
    /// function names to tuples of `(port_name, msb, lsb)`. For example, if
    /// `mapping` is `{"data": ("a_data", 3, 0), "valid": ("a_valid", 1, 1)}`,
    /// this defines an interface with two functions, `data` and `valid`, where
    /// the `data` function is provided by the port slice `a_data[3:0]` and the
    /// `valid` function is provided by the port slice `[1:1]`.
    pub fn def_intf(
        &self,
        name: impl AsRef<str>,
        mapping: IndexMap<String, (String, usize, usize)>,
    ) -> Intf {
        let members = mapping
            .iter()
            .map(|(func_name, (port_name, msb, lsb))| {
                IntfMember::range(func_name, port_name, *msb, *lsb)
            })
            .collect();
        self.define_intf(name.as_ref(), members)
    }

    /// Defines an interface from `(member, slice)` pairs, in order. Slices
    /// keep their typed selection, so members built with
    /// [`PortSlice::index`] or [`PortSlice::field`] stay typed. A member name
    /// may appear twice, once per port direction: `connect` pairs each
    /// member with the same-named member of the other interface driven the
    /// other way, so that a link `{req: req_o[d], req: req_i[d]}` connects
    /// to a neighbour's link output to input in both directions.
    ///
    /// Panics if a slice is not on a port of this module definition or no
    /// member is given, and with S0208 if a name repeats in one direction.
    #[track_caller]
    pub fn def_intf_members<S: Into<String>>(
        &self,
        name: impl AsRef<str>,
        members: impl IntoIterator<Item = (S, PortSlice)>,
    ) -> Intf {
        let name = name.as_ref();
        let members: Vec<(String, PortSlice)> =
            members.into_iter().map(|(n, s)| (n.into(), s)).collect();
        assert!(
            !members.is_empty(),
            "Empty interface definition for {}.{}",
            self.get_name(),
            name
        );
        let this = Arc::downgrade(&self.core);
        for (member, slice) in &members {
            let on_this = matches!(
                &slice.port.kind,
                PortKind::ModDef { mod_def_core, .. } if Weak::ptr_eq(mod_def_core, &this)
            );
            assert!(
                on_this,
                "Member {member} of interface {}.{name} is {}, not a port of {}",
                self.get_name(),
                slice.debug_string(),
                self.get_name()
            );
        }
        for (i, (member, slice)) in members.iter().enumerate() {
            let same_direction = |other: &PortSlice| {
                std::mem::discriminant(&other.port.io()) == std::mem::discriminant(&slice.port.io())
            };
            if let Some((_, first)) = members[..i]
                .iter()
                .find(|(m, s)| m == member && same_direction(s))
            {
                Diagnostic {
                    code: codes::S0208,
                    severity: Severity::Error,
                    message: format!(
                        "member `{member}` is an {} twice in interface `{}.{name}`",
                        direction(slice),
                        self.get_name()
                    ),
                    primary: Label {
                        span: Span::caller(),
                        text: "interface defined here".into(),
                    },
                    secondary: vec![],
                    notes: vec![format!(
                        "`{}` and `{}` are both {}s",
                        describe(first),
                        describe(slice),
                        direction(slice)
                    )],
                    helps: vec![
                        "a name may appear once per direction; give one of them another name"
                            .into(),
                    ],
                    object: None,
                }
                .panic();
            }
        }
        let members = members
            .iter()
            .map(|(member, slice)| IntfMember::new(member, slice))
            .collect();
        self.define_intf(name, members)
    }

    pub(crate) fn define_intf(&self, name: &str, members: Vec<IntfMember>) -> Intf {
        let mut core = self.core.write();
        if core.interfaces.contains_key(name) {
            panic!("Interface {} already exists in module {}", name, core.name);
        }
        core.interfaces.insert(name.to_string(), members);
        Intf {
            kind: IntfKind::ModDef {
                name: name.to_string(),
                mod_def_core: Arc::downgrade(&self.core),
            },
        }
    }

    /// Defines an interface with the given name, where the function names are
    /// derived from the port names by stripping a common prefix. For example,
    /// if the module has ports `a_data`, `a_valid`, `b_data`, and `b_valid`,
    /// calling `def_intf_from_prefix("a_intf", "a_")` will define an interface
    /// with functions `data` and `valid`, where `data` is provided by the full
    /// port `a_data` and `valid` is provided by the full port `a_valid`.
    pub fn def_intf_from_prefix(&self, name: impl AsRef<str>, prefix: impl AsRef<str>) -> Intf {
        self.def_intf_from_prefixes(name, &[prefix.as_ref()], true)
    }

    /// Defines an interface with the given name, where the function names are
    /// derived from the port names by stripping the prefix `<name>_`. For
    /// example, if the module has ports `a_data`, `a_valid`, `b_data`, and
    /// `b_valid`, calling `def_intf_from_prefix("a")` will define an
    /// interface with functions `data` and `valid`, where `data` is provided by
    /// the full port `a_data` and `valid` is provided by the full port
    /// `a_valid`.
    pub fn def_intf_from_name_underscore(&self, name: impl AsRef<str>) -> Intf {
        let prefix = format!("{}_", name.as_ref());
        self.def_intf_from_prefix(name, prefix)
    }

    /// Defines an interface with the given name, where the signals to be
    /// included are identified by those that start with one of the provided
    /// prefixies. Function names are either the signal names themselves (if
    /// `strip_prefix` is `false`) or by stripping the prefix (if `strip_prefix`
    /// is true). For example, if the module has ports `a_data`, `a_valid`,
    /// `b_data`, and `b_valid`, calling `def_intf_from_prefixes("intf", &["a_",
    /// "b_"], false)` will define an interface with functions `a_data`,
    /// `a_valid`, `b_data`, and `b_valid`, where each function is provided by
    /// the corresponding port.
    pub fn def_intf_from_prefixes(
        &self,
        name: impl AsRef<str>,
        prefixes: &[&str],
        strip_prefix: bool,
    ) -> Intf {
        let mut mapping = IndexMap::new();
        {
            let core = self.core.read();
            for port_name in core.ports.keys() {
                for prefix in prefixes {
                    if port_name.starts_with(prefix) {
                        let func_name = if strip_prefix {
                            port_name.strip_prefix(prefix).unwrap().to_string()
                        } else {
                            port_name.clone()
                        };
                        let port = self.get_port(port_name);
                        if mapping.contains_key(&func_name) {
                            panic!(
                                "Interface function {}.{}.{} already exists",
                                core.name,
                                name.as_ref(),
                                func_name
                            );
                        }
                        mapping.insert(func_name, (port_name.clone(), port.io().width() - 1, 0));
                        break;
                    }
                }
            }
        }

        assert!(
            !mapping.is_empty(),
            "Empty interface definition for {}.{}",
            self.get_name(),
            name.as_ref()
        );

        self.def_intf(name, mapping)
    }

    pub fn def_intf_from_regex(
        &self,
        name: impl AsRef<str>,
        search: impl AsRef<str>,
        replace: impl AsRef<str>,
    ) -> Intf {
        self.def_intf_from_regexes(name, &[(search.as_ref(), replace.as_ref())])
    }

    pub fn def_intf_from_regexes(&self, name: impl AsRef<str>, regexes: &[(&str, &str)]) -> Intf {
        let mut mapping = IndexMap::new();
        let regexes = regexes
            .iter()
            .map(|(search, replace)| {
                (
                    Regex::new(search).expect("Failed to compile regex"),
                    replace,
                )
            })
            .collect::<Vec<_>>();
        {
            let core = self.core.read();
            for port_name in core.ports.keys() {
                for (regex, replace) in &regexes {
                    if regex.is_match(port_name) {
                        let func_name = regex.replace(port_name, **replace).to_string();
                        let port = self.get_port(port_name);
                        if mapping.contains_key(&func_name) {
                            panic!(
                                "Interface function {}.{}.{} already exists",
                                core.name,
                                name.as_ref(),
                                func_name
                            );
                        }
                        mapping.insert(func_name, (port_name.clone(), port.io().width() - 1, 0));
                        break;
                    }
                }
            }
        }

        assert!(
            !mapping.is_empty(),
            "Empty interface definition for {}.{}",
            self.get_name(),
            name.as_ref()
        );

        self.def_intf(name, mapping)
    }

    /// Returns the interface with the given name; panics if an interface with
    /// that name does not exist.
    pub fn get_intf(&self, name: impl AsRef<str>) -> Intf {
        let core = self.core.read();
        if core.interfaces.contains_key(name.as_ref()) {
            Intf {
                kind: IntfKind::ModDef {
                    name: name.as_ref().to_string(),
                    mod_def_core: Arc::downgrade(&self.core),
                },
            }
        } else {
            panic!(
                "Interface '{}' does not exist in module '{}'",
                name.as_ref(),
                core.name
            );
        }
    }

    /// Returns `true` if this module definition has an interface with the given
    /// name.
    pub fn has_intf(&self, name: impl AsRef<str>) -> bool {
        self.core.read().interfaces.contains_key(name.as_ref())
    }

    /// Returns a vector of all interfaces on this module definition with the
    /// given prefix. If `prefix` is `None`, returns all interfaces.
    pub fn get_intfs(&self, prefix: Option<&str>) -> Vec<Intf> {
        let inner = self.core.read();
        let mut result = Vec::new();
        for name in inner.interfaces.keys() {
            if prefix.is_none_or(|pfx| name.starts_with(pfx)) {
                result.push(Intf {
                    kind: IntfKind::ModDef {
                        name: name.clone(),
                        mod_def_core: Arc::downgrade(&self.core),
                    },
                });
            }
        }
        result
    }
}
