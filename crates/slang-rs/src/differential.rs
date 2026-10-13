// SPDX-License-Identifier: Apache-2.0

//! Compares the design snapshot with the answers of the JSON extractors on
//! every run, while both exist. Enabled by the `differential` feature, which
//! the dev-dependencies of the workspace's crates turn on; a mismatch panics
//! with every difference found.
//!
//! The JSON extractors' defects are the cases the snapshot fixes on purpose;
//! each is normalized away explicitly here and nowhere else (see
//! `doc/design/slang-in-process.md`, "Fixed behavior").

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;

use regex::Regex;
use slang_sys::{Design, Instance, Response};

use crate::extract::{
    TypeResolver, extract_instance_interface_ports_with,
    extract_instance_overridable_parameters_with, extract_instance_parameter_defs_with,
    extract_instance_ports_checked_with, instance_child_names_with, instance_has_ref_port_with,
    instance_unparsed_parameters_with, top_names_with,
};
use crate::hierarchy::json as json_hierarchy;
use crate::package::json;
use crate::{Ast, Field, ParameterDef, Port, Type, UnsupportedPort};

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
        self.packages();
        self.hierarchy();
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

    /// Compares the packages with their parameters' values. The snapshot
    /// prints an alias of a struct, union or enum with its package-qualified
    /// name (`...}my_pkg::my_struct_t`); the JSON path prints the bare name,
    /// except for an alias of an array, so the qualifier of the alias itself
    /// is removed from the snapshot's value when that makes them equal.
    fn packages(&mut self) {
        let json = json::extract_packages_from_value(self.value, &self.resolver);
        let typed = self.design.packages();
        let json_names: BTreeSet<_> = json.keys().map(String::as_str).collect();
        let typed_names: BTreeSet<_> = typed.keys().map(String::as_str).collect();
        if !self.same_names("packages", &json_names, &typed_names) {
            return;
        }
        for (name, json) in &json {
            let typed = &typed[name];
            let path = format!("package {name}");
            let json_names: BTreeSet<_> = json.parameters.keys().map(String::as_str).collect();
            let typed_names: BTreeSet<_> = typed.parameters.keys().map(String::as_str).collect();
            if !self.same_names(&path, &json_names, &typed_names) {
                continue;
            }
            for (parameter, json) in &json.parameters {
                let typed = &typed.parameters[parameter];
                let bare = typed
                    .value
                    .replace(&format!("{name}::{parameter}"), parameter);
                if json.value != typed.value && json.value != bare {
                    self.report(
                        &format!("{path}::{parameter}"),
                        "values differ",
                        &json.value,
                        &typed.value,
                    );
                }
            }
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
        self.parameters(path);
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

    fn parameters(&mut self, path: &str) {
        let json = extract_instance_parameter_defs_with(&self.resolver, self.value, path)
            .expect("the instance exists");
        let json_overridable =
            extract_instance_overridable_parameters_with(&self.resolver, self.value, path)
                .expect("the instance exists");
        let (Some(typed), Some(typed_overridable)) = (
            self.design.parameters(path),
            self.design.overridable_parameters(path),
        ) else {
            self.report(path, "instance missing from the design", &(), &());
            return;
        };
        if json_overridable != typed_overridable {
            self.report(
                path,
                "overridable parameters differ",
                &json_overridable,
                &typed_overridable,
            );
        }
        // A parameter typed by a typedef of a type parameter the JSON path
        // could not resolve (its text still has the link) is skipped there
        // and listed by the design.
        let unresolved: HashSet<_> =
            instance_unparsed_parameters_with(&self.resolver, self.value, path)
                .expect("the instance exists")
                .into_iter()
                .filter(|(_, type_text)| self.link.is_match(type_text))
                .map(|(name, _)| name)
                .collect();
        let typed: Vec<_> = typed
            .iter()
            .filter(|parameter| !unresolved.contains(&parameter.name))
            .cloned()
            .collect();
        if !self.same_parameters(&json, &typed) {
            self.report(path, "parameters differ", &json, &typed);
        }
    }

    fn same_parameters(&self, json: &[ParameterDef], typed: &[ParameterDef]) -> bool {
        json.len() == typed.len()
            && json
                .iter()
                .zip(typed)
                .all(|(json, typed)| json.name == typed.name && self.same_type(&json.ty, &typed.ty))
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

    /// Compares the instance trees. Two defects of the JSON path are
    /// normalized away: it names a loop element of a generate block by its
    /// position in the loop (`g[0]`, `g[1]`) where the design uses the
    /// genvar's value (`g[2]`, `g[3]`), so both sides' indices are replaced
    /// by the order they appear in under their instance (an element without
    /// instances is in neither); and it lists no element of an instance array
    /// (`u[3]`), so those are dropped from the design's trees.
    fn hierarchy(&mut self) {
        let json = json_hierarchy::extract_hierarchy_from_value(self.value);
        let typed = self.design.hierarchy();
        let json_names: BTreeSet<_> = json.keys().map(String::as_str).collect();
        let typed_names: BTreeSet<_> = typed.keys().map(String::as_str).collect();
        if !self.same_names("hierarchy", &json_names, &typed_names) {
            return;
        }
        for (name, json) in &json {
            let mut json_lines = Vec::new();
            flatten(json, "", 0, &mut json_lines);
            let mut typed_lines = Vec::new();
            flatten(&typed[name], "", 0, &mut typed_lines);
            if json_lines != typed_lines {
                self.report(
                    &format!("hierarchy of {name}"),
                    "instance trees differ",
                    &json_lines,
                    &typed_lines,
                );
            }
        }
    }
}

/// One line per instance, depth first: `depth definition name prefix`.
fn line(depth: usize, instance: &Instance, prefix: &str) -> String {
    format!(
        "{depth} {} {} {prefix}",
        instance.def_name, instance.inst_name
    )
}

/// The tree under `instance` as lines, with generate indices normalized
/// (see [`Checker::hierarchy`]) and array elements dropped.
fn flatten(instance: &Instance, prefix: &str, depth: usize, out: &mut Vec<String>) {
    out.push(line(depth, instance, prefix));
    let mut positions = Positions::default();
    for child in &instance.contents {
        let child = child.borrow();
        if child.inst_name.contains('[') {
            continue;
        }
        let prefix = positions.normalize(&child.hier_prefix);
        flatten(&child, &prefix, depth + 1, out);
    }
}

/// The indices of each loop seen so far under one instance, in order of
/// appearance; the key is the loop's normalized path.
#[derive(Default)]
struct Positions(HashMap<String, Vec<String>>);

impl Positions {
    /// Replaces each `[value]` of a generate prefix by the position of the
    /// value among those of that loop seen under the same instance.
    fn normalize(&mut self, prefix: &str) -> String {
        let mut normalized = String::new();
        for segment in prefix.split('.').filter(|s| !s.is_empty()) {
            let (name, value) = match segment.split_once('[') {
                Some((name, value)) => (name, Some(value.trim_end_matches(']'))),
                None => (segment, None),
            };
            normalized.push('.');
            normalized.push_str(name);
            if let Some(value) = value {
                let seen = self.0.entry(normalized.clone()).or_default();
                let position = seen.iter().position(|v| v == value).unwrap_or_else(|| {
                    seen.push(value.to_string());
                    seen.len() - 1
                });
                normalized.push_str(&format!("[{position}]"));
            }
        }
        normalized
    }
}
