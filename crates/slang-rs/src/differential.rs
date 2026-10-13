// SPDX-License-Identifier: Apache-2.0

//! Compares the design snapshot with the answers of the JSON extractors on
//! every run, while both exist. Enabled by the `differential` feature, which
//! the dev-dependencies of the workspace's crates turn on; a mismatch panics
//! with every difference found.
//!
//! The JSON extractors' defects are the cases the snapshot fixes on purpose;
//! each is normalized away explicitly here and nowhere else (see
//! `doc/design/slang-in-process.md`, "Fixed behavior").

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write;

use regex::Regex;
use slang_sys::{Design, Response};

use crate::extract::{
    TypeResolver, extract_instance_interface_ports_with, extract_instance_ports_checked_with,
    instance_child_names_with, instance_has_ref_port_with, top_names_with,
};
use crate::{Ast, Field, Port, Type, UnsupportedPort};

/// The snapshot's error text for a port that connects several signals; the
/// JSON extractors drop such ports.
const MULTI_PORT: &str = "ports that connect several signals are not supported";

type Ports = Vec<Result<Port, UnsupportedPort>>;

/// Both paths produce their result for the same runs: the AST is dumped
/// exactly when a design is built.
pub(crate) fn check_agreement(response: &Response) {
    assert_eq!(
        response.ast_json.is_empty(),
        response.design.is_none(),
        "the JSON AST and the design disagree on whether the run produced a result"
    );
}

/// Panics if the design differs from the JSON AST in the areas that read the
/// design: the tops, the instances below them, and their ports.
pub(crate) fn check(ast: &Ast, design: &Design) {
    let mut checker = Checker {
        value: ast,
        design,
        resolver: TypeResolver::new(ast),
        link: Regex::new(r"\b[0-9]+ [A-Za-z_$]").unwrap(),
        type_parameter_aliases: type_parameter_aliases(ast),
        differences: String::new(),
    };
    checker.run();
    assert!(
        checker.differences.is_empty(),
        "the design differs from the JSON AST:\n{}",
        checker.differences
    );
}

struct Checker<'a> {
    value: &'a serde_json::Value,
    design: &'a Design,
    resolver: TypeResolver<'a>,
    /// An unresolved type link (`123 name`) left in an unsupported type's text.
    link: Regex,
    /// See [`type_parameter_aliases`].
    type_parameter_aliases: HashSet<String>,
    differences: String,
}

/// The printed names (`c.tt`) of the typedefs that alias a type parameter of
/// their module without adding a dimension (`typedef T tt;`), directly or
/// through other such typedefs (`typedef tt tt2;`). The snapshot
/// names a struct, union or enum reached through one by the typedef; the
/// JSON path names it by the type parameter's value.
fn type_parameter_aliases(value: &serde_json::Value) -> HashSet<String> {
    let mut aliases = HashSet::new();
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            serde_json::Value::Object(object) => {
                if object.get("kind").and_then(|k| k.as_str()) == Some("InstanceBody") {
                    body_type_parameter_aliases(value, &mut aliases);
                }
                stack.extend(object.values());
            }
            serde_json::Value::Array(array) => stack.extend(array),
            _ => {}
        }
    }
    aliases
}

fn body_type_parameter_aliases(body: &serde_json::Value, aliases: &mut HashSet<String>) {
    let (Some(body_name), Some(members)) = (body["name"].as_str(), body["members"].as_array())
    else {
        return;
    };
    let members_of = |kind: &'static str| {
        members
            .iter()
            .filter(move |m| m["kind"] == kind)
            .filter_map(|m| Some((m["name"].as_str()?, m)))
    };
    // The type parameters, then the aliases that link to one of these
    // without a dimension (`123 c.T`, `123 c.tt`), until none is added.
    let mut reaching: HashSet<_> = members_of("TypeParameter").map(|(name, _)| name).collect();
    loop {
        let added: Vec<_> = members_of("TypeAlias")
            .filter(|(name, alias)| {
                !reaching.contains(name)
                    && alias["target"]
                        .as_str()
                        .and_then(|target| target.split_once(' '))
                        .and_then(|(_, linked)| linked.strip_prefix(body_name))
                        .and_then(|rest| rest.strip_prefix('.'))
                        .is_some_and(|linked| reaching.contains(linked))
            })
            .map(|(name, _)| name)
            .collect();
        if added.is_empty() {
            break;
        }
        for name in added {
            reaching.insert(name);
            aliases.insert(format!("{body_name}.{name}"));
        }
    }
}

impl Checker<'_> {
    fn run(&mut self) {
        let json_tops = top_names_with(&self.resolver, self.value);
        let json: BTreeSet<_> = json_tops.iter().map(String::as_str).collect();
        let typed: BTreeSet<_> = self.design.tops().collect();
        if !self.same_names("tops", &json, &typed) {
            return;
        }
        for top in json {
            self.instance(top);
        }
    }

    fn report(
        &mut self,
        path: &str,
        what: &str,
        json: &dyn std::fmt::Debug,
        typed: &dyn std::fmt::Debug,
    ) {
        writeln!(
            self.differences,
            "{path}: {what}\n  json:   {json:?}\n  design: {typed:?}"
        )
        .unwrap();
    }

    fn same_names(&mut self, path: &str, json: &BTreeSet<&str>, typed: &BTreeSet<&str>) -> bool {
        if json == typed {
            return true;
        }
        let only_json: Vec<_> = json.difference(typed).collect();
        let only_typed: Vec<_> = typed.difference(json).collect();
        self.report(path, "names differ", &only_json, &only_typed);
        false
    }

    /// Compares the instance at `path` and everything below it.
    fn instance(&mut self, path: &str) {
        let Some(has_ref) = instance_has_ref_port_with(&self.resolver, self.value, path) else {
            self.report(path, "instance missing from the JSON AST", &(), &());
            return;
        };
        // The JSON extractor panics on a `ref` port.
        if !has_ref {
            self.ports(path);
        }
        self.children(path);
    }

    fn ports(&mut self, path: &str) {
        let json = extract_instance_ports_checked_with(&self.resolver, self.value, path)
            .expect("the instance exists");
        let json_interface =
            extract_instance_interface_ports_with(&self.resolver, self.value, path)
                .expect("the instance exists");
        let (Some(typed), Some(typed_interface)) =
            (self.design.ports(path), self.design.interface_ports(path))
        else {
            self.report(path, "instance missing from the design", &(), &());
            return;
        };
        if json_interface != typed_interface {
            self.report(
                path,
                "interface ports differ",
                &json_interface,
                &typed_interface,
            );
        }
        let typed: Ports = typed
            .iter()
            .filter(|port| !matches!(port, Err(unsupported) if unsupported.error == MULTI_PORT))
            .cloned()
            .collect();
        if !self.same_ports(&json, &typed) {
            self.report(path, "ports differ", &json, &typed);
        }
    }

    fn same_ports(&self, json: &Ports, typed: &Ports) -> bool {
        json.len() == typed.len()
            && json.iter().zip(typed).all(|pair| match pair {
                (Ok(json), Ok(typed)) => {
                    json.dir == typed.dir
                        && json.name == typed.name
                        && json.loc == typed.loc
                        && self.same_type(&json.ty, &typed.ty)
                }
                // The error texts are Slang's display of the type in the
                // snapshot; the JSON path's differ.
                (Err(json), Err(typed)) => json.name == typed.name && json.loc == typed.loc,
                // A typedef of a type parameter the JSON path could not
                // resolve; its text still has the link.
                (Err(json), Ok(_)) => self.link.is_match(&json.type_text),
                (Ok(_), Err(_)) => false,
            })
    }

    /// Equal, except where the snapshot names a type by a typedef of a type
    /// parameter (see [`type_parameter_aliases`]).
    fn same_type(&self, json: &Type, typed: &Type) -> bool {
        let same_name =
            |json: &str, typed: &str| json == typed || self.type_parameter_aliases.contains(typed);
        let same_fields = |json: &[Field], typed: &[Field]| {
            json.len() == typed.len()
                && json
                    .iter()
                    .zip(typed)
                    .all(|(j, t)| j.name == t.name && self.same_type(&j.ty, &t.ty))
        };
        match (json, typed) {
            (
                Type::Struct {
                    name: jn,
                    signed: js,
                    packed: jp,
                    fields: jf,
                    packed_dimensions: jpd,
                    unpacked_dimensions: jud,
                },
                Type::Struct {
                    name: tn,
                    signed: ts,
                    packed: tp,
                    fields: tf,
                    packed_dimensions: tpd,
                    unpacked_dimensions: tud,
                },
            )
            | (
                Type::Union {
                    name: jn,
                    signed: js,
                    packed: jp,
                    fields: jf,
                    packed_dimensions: jpd,
                    unpacked_dimensions: jud,
                },
                Type::Union {
                    name: tn,
                    signed: ts,
                    packed: tp,
                    fields: tf,
                    packed_dimensions: tpd,
                    unpacked_dimensions: tud,
                },
            ) => {
                same_name(jn, tn)
                    && js == ts
                    && jp == tp
                    && jpd == tpd
                    && jud == tud
                    && same_fields(jf, tf)
            }
            (
                Type::Enum {
                    name: jn,
                    variants: jv,
                    packed_dimensions: jpd,
                    unpacked_dimensions: jud,
                },
                Type::Enum {
                    name: tn,
                    variants: tv,
                    packed_dimensions: tpd,
                    unpacked_dimensions: tud,
                },
            ) => same_name(jn, tn) && jv == tv && jpd == tpd && jud == tud,
            _ => json == typed,
        }
    }

    fn children(&mut self, path: &str) {
        let json = instance_child_names_with(&self.resolver, self.value, path)
            .expect("the instance exists");
        let json: BTreeSet<_> = json.iter().map(String::as_str).collect();
        let Some(typed) = self.design.child_instances(path) else {
            self.report(path, "instance missing from the design", &(), &());
            return;
        };
        // The JSON path drops the elements of instance arrays.
        let typed: BTreeSet<_> = typed
            .into_iter()
            .filter(|name| !name.contains('['))
            .collect();
        if !self.same_names(&format!("{path} (children)"), &json, &typed) {
            return;
        }
        for name in json {
            self.instance(&format!("{path}.{name}"));
        }
    }
}
