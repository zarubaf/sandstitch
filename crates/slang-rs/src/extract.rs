// SPDX-License-Identifier: Apache-2.0

//! Extraction and compatibility support for Slang's serialized AST.
//!
//! Slang releases before v10 generally printed a complete textual type at each
//! use site in `--ast-json`. For example, a port declared with a packed struct
//! typedef could have a `type` field like:
//!
//! ```text
//! struct packed{logic[7:0] data;}bus_t
//! ```
//!
//! Starting in Slang v10, the serializer avoids repeating types that it has
//! already emitted. Later uses are encoded as an address followed by the type's
//! display name, for example `6338692580072 bus_t`. The address identifies an
//! object elsewhere in the same JSON document via that object's `addr` field.
//! Links can also occur inside other textual type definitions, so typedefs may
//! form a chain of references.
//!
//! The parser in [`type_extract`] consumes the older, self-contained textual
//! representation. [`TypeResolver`] bridges the formats by indexing addressable
//! JSON nodes and expanding v10+ links back into that representation before
//! parsing. Older inline types contain no links and pass through unchanged.

use num_bigint::BigInt;
use num_traits::Num;
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, hash_map::Entry};
use std::error::Error;
use std::hash::Hash;
use std::str::FromStr;

mod type_extract;
pub use slang_sys::{
    Field, InterfacePort, ParameterDef, ParameterKind, Port, PortDir, Range, SourceLoc, Type,
    UnsupportedPort, Variant,
};
pub use type_extract::parse_type_definition;

pub fn extract_ports(
    cfg: &crate::SlangConfig,
    skip_unsupported: bool,
) -> HashMap<String, Vec<Port>> {
    let result = crate::run_slang(cfg).unwrap().ast;
    extract_ports_from_value(&result, skip_unsupported)
}

struct MemberIter<'a> {
    members: Option<&'a Vec<Value>>,
    kinds: &'a [&'a str],
    current: usize,
}

impl<'a> MemberIter<'a> {
    fn new(value: &'a Value, kinds: &'a [&'a str]) -> Self {
        Self {
            members: value["members"].as_array(),
            kinds,
            current: 0,
        }
    }
}

impl<'a> Iterator for MemberIter<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(members) = self.members {
            while self.current < members.len() {
                let member = &members[self.current];
                let kind = member["kind"].as_str().unwrap();
                self.current += 1;
                if self.kinds.contains(&kind) {
                    return Some(member);
                }
            }
            None
        } else {
            None
        }
    }
}

fn parse_type_definition_no_error(type_str: &str) -> Result<Type, Box<dyn Error>> {
    if type_str == "<error>" {
        Err("Found \"<error>\" type in Slang JSON output.")?;
    }
    parse_type_definition(type_str)
}

fn insert_to_vacant<K, V>(map: &mut HashMap<K, V>, key: K, value: V) -> Result<(), String>
where
    K: Eq,
    K: Hash,
{
    match map.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(value);
            Ok(())
        }
        Entry::Occupied(_) => Err(String::from("Duplicate insertion")),
    }
}

/// Iterates over the top-level instances of the design, yielding each
/// instance's name together with its (link-resolved) body.
fn top_level_bodies<'a>(
    value: &'a Value,
    type_resolver: &TypeResolver<'a>,
) -> impl Iterator<Item = (&'a str, &'a Value)> {
    MemberIter::new(&value["design"], &["Instance"]).filter_map(move |member| {
        let module_name = member["name"].as_str().unwrap();
        if module_name.is_empty() {
            None
        } else {
            Some((module_name, type_resolver.resolve_node(&member["body"])))
        }
    })
}

/// Finds the body of the instance at a dot-separated hierarchical `path`,
/// starting from a top-level instance (e.g. `"top"` or `"top.u_child"`).
///
/// Only instances that are direct members of their parent's body are
/// followed; instances inside generate blocks are not reachable.
fn find_instance_body<'a>(
    value: &'a Value,
    type_resolver: &TypeResolver<'a>,
    path: &str,
) -> Option<&'a Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let (_, mut body) = top_level_bodies(value, type_resolver).find(|(name, _)| *name == first)?;
    for segment in segments {
        let instance = MemberIter::new(body, &["Instance"])
            .find(|member| member["name"].as_str() == Some(segment))?;
        body = type_resolver.resolve_node(&instance["body"]);
    }
    Some(body)
}

/// Reads the `source_*` fields added by `--ast-json-source-info`.
fn source_loc(node: &Value) -> Option<SourceLoc> {
    Some(SourceLoc {
        file: node.get("source_file")?.as_str()?.to_string(),
        line: u32::try_from(node.get("source_line")?.as_u64()?).ok()?,
        column: u32::try_from(node.get("source_column")?.as_u64()?)
            .ok()
            .filter(|column| *column != 0),
    })
}

/// Extracts the ports of one instance body. Port types that refer to the
/// body's type parameters resolve to the parameters' actual types.
///
/// Shared by [`extract_ports_from_value`] and
/// [`extract_instance_ports_from_value`], which differ only in how they find
/// the body.
fn body_ports(type_resolver: &TypeResolver, body: &Value, skip_unsupported: bool) -> Vec<Port> {
    let type_parameters = type_resolver.type_parameters(body);
    let mut ports = Vec::new();
    for instance_member in MemberIter::new(body, &["Port", "InterfacePort"]) {
        let kind = instance_member["kind"].as_str().unwrap();
        match kind {
            "Port" => match parse_port(type_resolver, &type_parameters, instance_member) {
                Ok(port) => ports.push(port),
                Err(unsupported) => {
                    if !skip_unsupported {
                        panic!("{}", unsupported.error);
                    }
                }
            },
            "InterfacePort" => {
                if !skip_unsupported {
                    panic!("Interface ports are not currently supported.")
                }
            }
            _ => continue,
        }
    }
    ports
}

/// Parses one `Port` member of an instance body.
fn parse_port(
    type_resolver: &TypeResolver,
    type_parameters: &HashMap<String, String>,
    member: &Value,
) -> Result<Port, UnsupportedPort> {
    let port_name = member["name"].as_str().unwrap();
    let direction = member["direction"].as_str().unwrap();
    let type_text = type_resolver.resolve_with(member["type"].as_str().unwrap(), type_parameters);
    match parse_type_definition_no_error(&type_text) {
        Ok(ty) => Ok(Port {
            dir: PortDir::from_str(direction).unwrap(),
            name: port_name.to_string(),
            ty,
            loc: source_loc(member),
        }),
        Err(e) => Err(UnsupportedPort {
            name: port_name.to_string(),
            type_text,
            error: e.to_string(),
            loc: source_loc(member),
        }),
    }
}

fn body_interface_ports(body: &Value) -> Vec<InterfacePort> {
    MemberIter::new(body, &["InterfacePort"])
        .map(|member| InterfacePort {
            name: member["name"].as_str().unwrap_or_default().to_string(),
            interface: member["interfaceDef"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            modport: member["modport"].as_str().map(str::to_string),
            loc: source_loc(member),
        })
        .collect()
}

/// Extracts the ports of every top-level module, keyed by module name.
///
/// Interface ports are not represented as [`Port`]s: they panic unless
/// `skip_unsupported` is set, in which case they are skipped; use
/// [`extract_interface_ports_from_value`] to list them.
pub fn extract_ports_from_value(
    value: &Value,
    skip_unsupported: bool,
) -> HashMap<String, Vec<Port>> {
    let mut ports_map = HashMap::new();
    let type_resolver = TypeResolver::new(value);

    for member in MemberIter::new(&value["design"], &["Instance"]) {
        let module_name = member["name"].as_str().unwrap();
        if module_name.is_empty() {
            continue;
        }
        let body = type_resolver.resolve_node(&member["body"]);
        let ports = body_ports(&type_resolver, body, skip_unsupported);
        insert_to_vacant(&mut ports_map, module_name.to_string(), ports)
            .unwrap_or_else(|_| panic!("Duplicate definition of module: {module_name}"));
    }

    ports_map
}

/// Extracts the ports of the instance at the dot-separated hierarchical
/// `path` (e.g. `"top.u_child"`), with that instance's parameter and type
/// parameter overrides applied. Returns `None` if no such instance exists.
///
/// Interface ports are handled as in [`extract_ports_from_value`].
pub fn extract_instance_ports_from_value(
    value: &Value,
    path: &str,
    skip_unsupported: bool,
) -> Option<Vec<Port>> {
    let type_resolver = TypeResolver::new(value);
    let body = find_instance_body(value, &type_resolver, path)?;
    Some(body_ports(&type_resolver, body, skip_unsupported))
}

/// Extracts the ports of the instance at the dot-separated hierarchical
/// `path`, like [`extract_instance_ports_from_value`], but never panics or
/// skips silently: a port whose type cannot be parsed is an
/// [`UnsupportedPort`]. Ports are in declaration order. Interface ports are
/// not included; use [`extract_instance_interface_ports_from_value`].
/// Returns `None` if no such instance exists.
pub fn extract_instance_ports_checked_from_value(
    value: &Value,
    path: &str,
) -> Option<Vec<Result<Port, UnsupportedPort>>> {
    extract_instance_ports_checked_with(&TypeResolver::new(value), value, path)
}

/// [`extract_instance_ports_checked_from_value`] with a resolver built once
/// for `value`.
pub(crate) fn extract_instance_ports_checked_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<Result<Port, UnsupportedPort>>> {
    let body = find_instance_body(value, type_resolver, path)?;
    let type_parameters = type_resolver.type_parameters(body);
    Some(
        MemberIter::new(body, &["Port"])
            .map(|member| parse_port(type_resolver, &type_parameters, member))
            .collect(),
    )
}

/// Whether the body of the instance at `path` has a `ref` port, which the
/// port extractors cannot read. `None` if no such instance exists.
#[cfg(feature = "differential")]
pub(crate) fn instance_has_ref_port_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<bool> {
    let body = find_instance_body(value, type_resolver, path)?;
    Some(MemberIter::new(body, &["Port"]).any(|member| member["direction"] == "Ref"))
}

/// The names of the instances that are direct members of the body of the
/// instance at `path` (not those in generate blocks or instance arrays).
/// `None` if no such instance exists.
#[cfg(feature = "differential")]
pub(crate) fn instance_child_names_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<String>> {
    let body = find_instance_body(value, type_resolver, path)?;
    Some(
        MemberIter::new(body, &["Instance"])
            .filter_map(|member| member["name"].as_str().map(str::to_string))
            .collect(),
    )
}

/// The names of the top-level instances.
#[cfg(feature = "differential")]
pub(crate) fn top_names_with(type_resolver: &TypeResolver, value: &Value) -> Vec<String> {
    top_level_bodies(value, type_resolver)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// Lists the interface ports of every top-level module, keyed by module name.
pub fn extract_interface_ports_from_value(value: &Value) -> HashMap<String, Vec<InterfacePort>> {
    let type_resolver = TypeResolver::new(value);
    top_level_bodies(value, &type_resolver)
        .map(|(module_name, body)| (module_name.to_string(), body_interface_ports(body)))
        .collect()
}

/// Lists the interface ports of the instance at the dot-separated
/// hierarchical `path`. Returns `None` if no such instance exists.
pub fn extract_instance_interface_ports_from_value(
    value: &Value,
    path: &str,
) -> Option<Vec<InterfacePort>> {
    extract_instance_interface_ports_with(&TypeResolver::new(value), value, path)
}

/// [`extract_instance_interface_ports_from_value`] with a resolver built
/// once for `value`.
pub(crate) fn extract_instance_interface_ports_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<InterfacePort>> {
    let body = find_instance_body(value, type_resolver, path)?;
    Some(body_interface_ports(body))
}

/// Lists the overridable type parameters (`parameter type T`) of every
/// top-level module, keyed by module name, in declaration order.
/// `localparam type` declarations are not included.
pub fn extract_type_parameter_names_from_value(value: &Value) -> HashMap<String, Vec<String>> {
    let type_resolver = TypeResolver::new(value);
    top_level_bodies(value, &type_resolver)
        .map(|(module_name, body)| {
            let names = MemberIter::new(body, &["TypeParameter"])
                .filter(|member| member["isLocal"].as_bool() != Some(true))
                .filter_map(|member| member["name"].as_str().map(str::to_string))
                .collect();
            (module_name.to_string(), names)
        })
        .collect()
}

/// Lists the overridable parameters (`parameter` and `parameter type`, not
/// `localparam`) of every top-level module, keyed by module name, in
/// declaration order.
pub fn extract_overridable_parameters_from_value(
    value: &Value,
) -> HashMap<String, Vec<(String, ParameterKind)>> {
    let type_resolver = TypeResolver::new(value);
    top_level_bodies(value, &type_resolver)
        .map(|(module_name, body)| (module_name.to_string(), body_overridable_parameters(body)))
        .collect()
}

/// The overridable parameters of the instance at `path`, with a resolver
/// built once for `value`. `None` if no such instance exists.
#[cfg(feature = "differential")]
pub(crate) fn extract_instance_overridable_parameters_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<(String, ParameterKind)>> {
    let body = find_instance_body(value, type_resolver, path)?;
    Some(body_overridable_parameters(body))
}

/// The value parameters of the instance at `path` that
/// [`extract_instance_parameter_defs_with`] skips, as `(name, type text)`.
/// `None` if no such instance exists.
#[cfg(feature = "differential")]
pub(crate) fn instance_unparsed_parameters_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<(String, String)>> {
    let body = find_instance_body(value, type_resolver, path)?;
    let type_parameters = type_resolver.type_parameters(body);
    Some(
        MemberIter::new(body, &["Parameter"])
            .filter_map(|member| {
                let type_str =
                    type_resolver.resolve_with(member["type"].as_str().unwrap(), &type_parameters);
                parse_type_definition_no_error(&type_str)
                    .is_err()
                    .then(|| Some((member["name"].as_str()?.to_string(), type_str)))?
            })
            .collect(),
    )
}

fn body_overridable_parameters(body: &Value) -> Vec<(String, ParameterKind)> {
    MemberIter::new(body, &["Parameter", "TypeParameter"])
        .filter(|member| member["isLocal"].as_bool() != Some(true))
        .filter_map(|member| {
            let kind = match member["kind"].as_str() {
                Some("TypeParameter") => ParameterKind::Type,
                _ => ParameterKind::Value,
            };
            Some((member["name"].as_str()?.to_string(), kind))
        })
        .collect()
}

/// Extracts the value parameters (localparams included) of the instance at
/// the dot-separated hierarchical `path`, with that instance's overrides
/// applied, as [`extract_parameter_defs_from_value`] does for top-level
/// modules. Parameters whose type cannot be parsed are skipped. Returns
/// `None` if no such instance exists.
pub fn extract_instance_parameter_defs_from_value(
    value: &Value,
    path: &str,
) -> Option<Vec<ParameterDef>> {
    extract_instance_parameter_defs_with(&TypeResolver::new(value), value, path)
}

/// [`extract_instance_parameter_defs_from_value`] with a resolver built once
/// for `value`.
pub(crate) fn extract_instance_parameter_defs_with(
    type_resolver: &TypeResolver,
    value: &Value,
    path: &str,
) -> Option<Vec<ParameterDef>> {
    let body = find_instance_body(value, type_resolver, path)?;
    let type_parameters = type_resolver.type_parameters(body);
    Some(
        MemberIter::new(body, &["Parameter"])
            .filter_map(|member| {
                let type_str =
                    type_resolver.resolve_with(member["type"].as_str().unwrap(), &type_parameters);
                Some(ParameterDef {
                    name: member["name"].as_str()?.to_string(),
                    ty: parse_type_definition_no_error(&type_str).ok()?,
                })
            })
            .collect(),
    )
}

pub fn extract_parameter_defs_from_value(
    value: &Value,
    skip_unsupported: bool,
) -> HashMap<String, Vec<ParameterDef>> {
    let mut parameters_map = HashMap::new();
    let type_resolver = TypeResolver::new(value);

    for member in MemberIter::new(&value["design"], &["Instance"]) {
        let module_name = member["name"].as_str().unwrap();
        if module_name.is_empty() {
            continue;
        }
        let mut parameters = Vec::new();
        let body = type_resolver.resolve_node(&member["body"]);
        // Type parameters (`TypeParameter` nodes) are not value parameters and
        // are not listed; see `extract_type_parameter_names_from_value`.
        let type_parameters = type_resolver.type_parameters(body);
        for instance_member in MemberIter::new(body, &["Parameter"]) {
            let parameter_name = instance_member["name"].as_str().unwrap();
            let type_str = type_resolver
                .resolve_with(instance_member["type"].as_str().unwrap(), &type_parameters);
            match parse_type_definition_no_error(&type_str) {
                Ok(ty) => parameters.push(ParameterDef {
                    name: parameter_name.to_string(),
                    ty,
                }),
                Err(e) => {
                    if skip_unsupported {
                        continue;
                    } else {
                        panic!("{}", e);
                    }
                }
            }
        }
        insert_to_vacant(&mut parameters_map, module_name.to_string(), parameters)
            .unwrap_or_else(|_| panic!("Duplicate definition of module: {module_name}"));
    }

    parameters_map
}

pub fn extract_parameter_defs(
    cfg: &crate::SlangConfig,
    skip_unsupported: bool,
) -> HashMap<String, Vec<ParameterDef>> {
    let result = crate::run_slang(cfg).unwrap().ast;
    extract_parameter_defs_from_value(&result, skip_unsupported)
}

pub fn extract_modules_from_value(value: &Value) -> Result<Vec<String>, Box<dyn Error>> {
    let definitions = value
        .get("definitions")
        .and_then(|v| v.as_array())
        .ok_or("JSON parsing failed")?;

    let mut modules = Vec::new();

    for definition in definitions {
        if definition.get("kind").and_then(|v| v.as_str()) == Some("Definition")
            && definition.get("definitionKind").and_then(|v| v.as_str()) == Some("Module")
        {
            if let Some(name) = definition.get("name").and_then(|v| v.as_str()) {
                modules.push(name.to_string());
            }
        }
    }

    Ok(modules)
}

pub fn extract_modules(cfg: &crate::SlangConfig) -> Result<Vec<String>, Box<dyn Error>> {
    let result = crate::run_slang(cfg)?.ast;
    extract_modules_from_value(&result)
}

/// Expands the address-based type links emitted by Slang v10 and newer back
/// into the self-contained textual format consumed by this crate's type parser.
///
/// Slang links have the form `<address> <display-name>`, where `<address>`
/// matches an `addr` property elsewhere in the same AST. The display name is
/// significant: it carries the user-visible typedef name, package qualification,
/// and any dimensions applied at the use site. The referenced node contains the
/// underlying definition.
///
/// A resolver borrows the AST because its address index stores references to
/// nodes in that document. Construct one resolver per AST and reuse it for all
/// type fields extracted from that AST.
pub(crate) struct TypeResolver<'a> {
    /// Every JSON object with an `addr` property, keyed by that Slang address.
    by_address: HashMap<u64, &'a Value>,
    /// Finds links both at the start of a type and nested within another type.
    link_pattern: Regex,
}

impl<'a> TypeResolver<'a> {
    /// Builds an address index for all objects reachable from `root`.
    ///
    /// Slang links are not necessarily local to the design node being examined;
    /// a type use can refer to a definition elsewhere in the document. Indexing
    /// the entire document up front makes subsequent resolution deterministic.
    pub(crate) fn new(root: &'a Value) -> Self {
        let mut by_address = HashMap::new();
        Self::index(root, &mut by_address);
        Self {
            by_address,
            link_pattern: Regex::new(
                r"(?P<addr>[0-9]+) (?P<name>[A-Za-z_$][A-Za-z0-9_$:.]*(?:\[[^\]]+\])*(?:\$(?:\[[^\]]+\])*)?)",
            )
            .unwrap(),
        }
    }

    /// Follows an address link to a structured AST node when Slang has replaced
    /// the inline object with a string such as `"123456 module_name"`.
    ///
    /// Instance bodies are shared this way in Slang v11. Inline objects from
    /// older Slang releases are returned unchanged, as are malformed or unknown
    /// links, so callers retain the pre-v11 behavior for those inputs.
    pub(crate) fn resolve_node(&self, value: &'a Value) -> &'a Value {
        let Some(address) = value
            .as_str()
            .and_then(|link| link.split_whitespace().next())
            .and_then(|address| address.parse::<u64>().ok())
        else {
            return value;
        };
        self.by_address.get(&address).copied().unwrap_or(value)
    }

    /// Records addressable objects without making assumptions about which AST
    /// scopes can contain type definitions. Iterative: the AST can be nested
    /// deeper than the caller's stack allows.
    fn index(root: &'a Value, by_address: &mut HashMap<u64, &'a Value>) {
        let mut stack = vec![root];
        while let Some(value) = stack.pop() {
            match value {
                Value::Object(object) => {
                    if let Some(address) = object.get("addr").and_then(Value::as_u64) {
                        by_address.insert(address, value);
                    }
                    stack.extend(object.values());
                }
                Value::Array(array) => stack.extend(array),
                _ => {}
            }
        }
    }

    /// Expands every resolvable Slang link in a textual type definition.
    ///
    /// Resolution is iterative because replacing an outer typedef link can
    /// expose more links inside its target. Unknown addresses and unsupported
    /// node kinds are preserved verbatim so the normal type parser can report
    /// an informative error (or the caller can skip unsupported types).
    pub(crate) fn resolve(&self, type_text: &str) -> String {
        self.resolve_with(type_text, &HashMap::new())
    }

    /// Collects the type parameters declared directly in an instance body,
    /// keyed by the scope-qualified name Slang uses when linking to them
    /// (`<body name>.<parameter name>`, e.g. `child.T`), with their actual
    /// (possibly overridden) type text as value.
    ///
    /// Slang links a use of a type parameter to the parameter's internal
    /// alias type, which is not itself serialized, so such links cannot be
    /// resolved by address. The `TypeParameter` node carries the type.
    pub(crate) fn type_parameters(&self, body: &Value) -> HashMap<String, String> {
        let Some(scope) = body["name"].as_str() else {
            return HashMap::new();
        };
        MemberIter::new(body, &["TypeParameter"])
            .filter_map(|member| {
                let name = member["name"].as_str()?;
                let ty = member["type"].as_str()?;
                Some((format!("{scope}.{name}"), ty.to_string()))
            })
            .collect()
    }

    /// Like [`Self::resolve`], additionally substituting links to the given
    /// type parameters (as collected by [`Self::type_parameters`]).
    pub(crate) fn resolve_with(
        &self,
        type_text: &str,
        type_parameters: &HashMap<String, String>,
    ) -> String {
        self.resolve_with_depth(type_text, type_parameters, 0)
    }

    fn resolve_with_depth(
        &self,
        type_text: &str,
        type_parameters: &HashMap<String, String>,
        depth: usize,
    ) -> String {
        let mut result = type_text.to_string();
        // Cap expansion to guard against malformed or cyclic linked typedefs.
        for _ in 0..64 {
            let replaced = self
                .link_pattern
                .replace_all(&result, |captures: &regex::Captures| {
                    let address = captures["addr"].parse::<u64>().unwrap();
                    self.render_link(address, &captures["name"])
                        .or_else(|| {
                            self.render_type_parameter(&captures["name"], type_parameters, depth)
                        })
                        .unwrap_or_else(|| captures[0].to_string())
                })
                .into_owned();
            if replaced == result {
                return result;
            }
            result = replaced;
        }
        result
    }

    /// Resolves a `TypeAlias` node that appears directly in a package.
    ///
    /// Direct package members are definitions rather than address-link use
    /// sites, so Slang does not repeat the public alias name in `target`. Pass
    /// that name explicitly to replace any generated aggregate name while
    /// preserving the same behavior as an ordinary linked alias.
    pub(crate) fn resolve_alias(&self, node: &Value, display_name: &str) -> Option<String> {
        let target = self.resolve(node.get("target")?.as_str()?);
        self.render_alias_target(&target, display_name)
    }

    /// Renders one linked definition using the name and dimensions from its use
    /// site rather than the serializer's internal generated name.
    fn render_link(&self, address: u64, display_name: &str) -> Option<String> {
        let node = *self.by_address.get(&address)?;
        match node.get("kind").and_then(Value::as_str)? {
            "TypeAlias" => self.resolve_alias(node, display_name),
            "EnumType" => self.render_enum(node, display_name),
            _ => None,
        }
    }

    /// Renders a link to a type parameter (`child.T[2:0]`) as the parameter's
    /// actual type with the use-site dimensions applied. The actual type keeps
    /// its own name (e.g. `cfg_pkg::flit_t`); the parameter name is dropped.
    ///
    /// Use-site dimensions are the outer ones: `T [2:0]` with `T = logic[7:0]`
    /// is `logic[2:0][7:0]`.
    fn render_type_parameter(
        &self,
        display_name: &str,
        type_parameters: &HashMap<String, String>,
        depth: usize,
    ) -> Option<String> {
        // A type parameter's type can name another type parameter of the
        // same scope; SystemVerilog forbids cycles, but guard regardless.
        if depth >= 16 {
            return None;
        }
        let dimension_index = dimension_start(display_name);
        let (name, use_site_dimensions) = match dimension_index {
            Some(index) => (&display_name[..index], &display_name[index..]),
            None => (display_name, ""),
        };
        let actual = type_parameters.get(name)?;
        let actual = self.resolve_with_depth(actual, type_parameters, depth + 1);
        // Aggregates end in their nominal name; their own dimensions follow it.
        let base_end = if is_aggregate(&actual) {
            let close = actual.rfind('}')?;
            close + 1 + dimension_start(&actual[close + 1..]).unwrap_or(actual.len() - close - 1)
        } else {
            dimension_start(&actual).unwrap_or(actual.len())
        };
        let (base, actual_dimensions) = actual.split_at(base_end);
        Some(format!(
            "{base}{}",
            merge_dimensions(use_site_dimensions, actual_dimensions)
        ))
    }

    /// Applies a typedef's public name and use-site dimensions to its resolved
    /// target text.
    fn render_alias_target(&self, target: &str, display_name: &str) -> Option<String> {
        let display_dimension_start = dimension_start(display_name);
        let display_dimensions = display_dimension_start
            .map(|index| &display_name[index..])
            .unwrap_or("");
        let display_name = display_dimension_start
            .map(|index| &display_name[..index])
            .unwrap_or(display_name);
        if is_aggregate(target) {
            // Aggregate targets end in an internal nominal name. Replace it
            // with the package-qualified name from this use site. Dimensions
            // after the target's internal name belong to the typedef itself and
            // must precede dimensions applied at this particular use site.
            //
            // The name names the element type, never the dimensions: when
            // the typedef adds dimensions (`typedef flit_t [1:0] pair_t`),
            // the alias name denotes the array, so the target's own (element)
            // name is kept.
            let close = target.rfind('}')?;
            let target_suffix = &target[close + 1..];
            let (target_name, target_dimensions) = match dimension_start(target_suffix) {
                Some(index) => target_suffix.split_at(index),
                None => (target_suffix, ""),
            };
            let name = if target_dimensions.is_empty() {
                display_name
            } else {
                target_name
            };
            Some(format!(
                "{}{name}{}",
                &target[..=close],
                merge_dimensions(target_dimensions, display_dimensions)
            ))
        } else {
            let target_dimension_start = dimension_start(target);
            let target_dimensions = target_dimension_start
                .map(|index| &target[index..])
                .unwrap_or("");
            let target = target_dimension_start
                .map(|index| &target[..index])
                .unwrap_or(target);
            Some(format!(
                "{target}{}",
                merge_dimensions(target_dimensions, display_dimensions)
            ))
        }
    }

    /// Reconstructs the legacy textual enum syntax from a v10+ `EnumType` node.
    ///
    /// Unlike struct and union aliases, Slang no longer leaves a complete enum
    /// declaration in a string that can simply be followed. Its members and
    /// values must therefore be rendered from the structured JSON node.
    fn render_enum(&self, node: &Value, display_name: &str) -> Option<String> {
        let members = node.get("members")?.as_array()?;
        let variants = members
            .iter()
            .filter(|member| member.get("kind").and_then(Value::as_str) == Some("EnumValue"))
            .map(|member| {
                let name = member.get("name")?.as_str()?;
                let value = member.get("value")?.as_str()?;
                let (width, signed, magnitude) = parse_integer_literal(value)?;
                let sign = if magnitude.sign() == num_bigint::Sign::Minus {
                    "-"
                } else {
                    ""
                };
                let signed = if signed { "s" } else { "" };
                Some(format!(
                    "{name}={sign}{width}'{signed}d{}",
                    magnitude.magnitude()
                ))
            })
            .collect::<Option<Vec<_>>>()?
            .join(",");
        Some(format!("enum{{{variants}}}{display_name}"))
    }
}

/// Whether a textual type is a struct, union or enum declaration (which ends
/// in a nominal name, followed by any dimensions).
/// The prefix may carry `packed` and a signing (`struct packed signed{`).
fn is_aggregate(type_text: &str) -> bool {
    matches!(
        type_text
            .split_once('{')
            .and_then(|(prefix, _)| prefix.split_whitespace().next()),
        Some("struct") | Some("union") | Some("enum")
    )
}

/// Finds the first packed (`[... ]`) or unpacked (`$[...]`) dimension.
///
/// Slang's generated nominal type names can themselves contain `$`, so only
/// `$[` marks an unpacked dimension boundary.
fn dimension_start(type_name: &str) -> Option<usize> {
    match (type_name.find('['), type_name.find("$[")) {
        (Some(packed), Some(unpacked)) => Some(packed.min(unpacked)),
        (Some(packed), None) => Some(packed),
        (None, Some(unpacked)) => Some(unpacked),
        (None, None) => None,
    }
}

/// Combines typedef and use-site dimensions into the parser's canonical form.
///
/// Slang writes a `$` before each independently serialized set of unpacked
/// dimensions. Once nested aliases are expanded there must instead be one `$`
/// separating all packed dimensions from all unpacked dimensions.
fn merge_dimensions(target: &str, display: &str) -> String {
    fn split(dimensions: &str) -> (&str, &str) {
        dimensions
            .find("$[")
            .map(|index| (&dimensions[..index], &dimensions[index + 1..]))
            .unwrap_or((dimensions, ""))
    }

    let (target_packed, target_unpacked) = split(target);
    let (display_packed, display_unpacked) = split(display);
    let unpacked = format!("{target_unpacked}{display_unpacked}");
    let separator = if unpacked.is_empty() { "" } else { "$" };
    format!("{target_packed}{display_packed}{separator}{unpacked}")
}

/// Parses a SystemVerilog integer emitted by Slang.
///
/// The AST can use binary, octal, decimal, or hexadecimal digits, while the
/// existing enum grammar expects decimal text. Signed bit patterns are converted
/// from their fixed-width two's-complement representation before rendering.
/// Slang v11 can also emit unsized enum values as plain decimal strings; these
/// have SystemVerilog's default signed 32-bit integer representation.
fn parse_integer_literal(value: &str) -> Option<(usize, bool, BigInt)> {
    let Some((width, digits)) = value.split_once('\'') else {
        let digits = value.replace('_', "");
        return Some((32, true, BigInt::from_str_radix(&digits, 10).ok()?));
    };
    let (negative, width) = match width.strip_prefix('-') {
        Some(width) => (true, width),
        None => (false, width),
    };
    let width = width.parse::<usize>().ok()?;
    let (signed, radix_char, digits) = if let Some(rest) = digits.strip_prefix('s') {
        (true, rest.chars().next()?, &rest[1..])
    } else {
        (false, digits.chars().next()?, &digits[1..])
    };
    let radix = match radix_char {
        'b' => 2,
        'o' => 8,
        'd' => 10,
        'h' => 16,
        _ => return None,
    };
    let digits = digits.replace('_', "");
    let mut number = BigInt::from_str_radix(&digits, radix).ok()?;
    if negative {
        number = -number;
    } else if signed && width > 0 && number.bit((width - 1) as u64) {
        number -= BigInt::from(1u8) << width;
    }
    Some((width, signed, number))
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_slang_v11_typedef_and_enum_links() {
        let ast = json!({
            "nodes": [
                {
                    "name": "bus_t",
                    "kind": "TypeAlias",
                    "addr": 100,
                    "target": "struct packed{logic[7:0] data;logic valid;}generated$1"
                },
                {
                    "name": "color_t",
                    "kind": "EnumType",
                    "addr": 200,
                    "members": [
                        { "name": "RED", "kind": "EnumValue", "value": "2'b0" },
                        { "name": "BLUE", "kind": "EnumValue", "value": "2'b10" }
                    ]
                }
            ]
        });
        let resolver = TypeResolver::new(&ast);

        assert_eq!(
            resolver.resolve("100 bus_t[3:0]"),
            "struct packed{logic[7:0] data;logic valid;}bus_t[3:0]"
        );
        assert_eq!(
            resolver.resolve_alias(&ast["nodes"][0], "pkg::bus_t"),
            Some("struct packed{logic[7:0] data;logic valid;}pkg::bus_t".to_string())
        );
        assert_eq!(
            resolver.resolve("200 pkg::color_t"),
            "enum{RED=2'd0,BLUE=2'd2}pkg::color_t"
        );
    }

    #[test]
    fn resolves_nested_slang_v11_typedef_links() {
        let ast = json!({
            "nodes": [
                {
                    "kind": "TypeAlias",
                    "addr": 100,
                    "target": "struct packed{logic[7:0] data;}generated$1"
                },
                {
                    "kind": "TypeAlias",
                    "addr": 200,
                    "target": "struct packed{100 inner_t value;}generated$2"
                },
                {
                    "kind": "TypeAlias",
                    "addr": 300,
                    "target": "100 inner_t[3:0]"
                },
                {
                    "kind": "TypeAlias",
                    "addr": 400,
                    "target": "logic$[3:0]"
                },
                {
                    "kind": "TypeAlias",
                    "addr": 500,
                    "target": "400 unpacked_t$[1:0]"
                }
            ]
        });
        let resolver = TypeResolver::new(&ast);

        assert_eq!(
            resolver.resolve("200 outer_t"),
            "struct packed{struct packed{logic[7:0] data;}inner_t value;}outer_t"
        );
        // An array typedef keeps its element's name: `outer_array_t` names
        // `inner_t [3:0]`, not the element.
        assert_eq!(
            resolver.resolve("300 outer_array_t[1:0]"),
            "struct packed{logic[7:0] data;}inner_t[3:0][1:0]"
        );
        assert_eq!(resolver.resolve("400 unpacked_t$[1:0]"), "logic$[3:0][1:0]");
        assert!(parse_type_definition(&resolver.resolve("400 unpacked_t$[1:0]")).is_ok());
        assert_eq!(resolver.resolve("500 nested_t"), "logic$[3:0][1:0]");
    }

    #[test]
    fn resolves_slang_v11_dotted_scope_qualified_links() {
        let ast = json!({
            "nodes": [{
                "kind": "TypeAlias",
                "addr": 100,
                "target": "struct packed{logic[7:0] data;}generated$1"
            }]
        });
        let resolver = TypeResolver::new(&ast);
        let resolved = resolver.resolve("100 top.byte_t");

        assert_eq!(resolved, "struct packed{logic[7:0] data;}top.byte_t");
        assert!(parse_type_definition(&resolved).is_ok());
    }

    #[test]
    fn resolves_slang_v11_linked_instance_bodies() {
        let ast = json!({
            "design": {
                "members": [
                    {
                        "kind": "InstanceBody",
                        "addr": 100,
                        "members": [
                            {
                                "kind": "Port",
                                "name": "data",
                                "direction": "In",
                                "type": "logic[7:0]"
                            },
                            {
                                "kind": "Parameter",
                                "name": "WIDTH",
                                "type": "int"
                            }
                        ]
                    },
                    { "kind": "Instance", "name": "top", "body": "100 top" }
                ]
            }
        });

        let ports = extract_ports_from_value(&ast, false);
        assert_eq!(ports["top"][0].name, "data");
        let parameters = extract_parameter_defs_from_value(&ast, false);
        assert_eq!(parameters["top"][0].name, "WIDTH");
    }

    #[test]
    fn resolves_slang_v11_type_parameter_links() {
        // Shape of Slang v11 output for
        // `child #(.T(cfg_pkg::flit_t), .N(3)) u();` with port
        // `input T [N-1:0] in_i`: the port links to an unserialized address.
        let ast = json!({
            "nodes": [{
                "name": "flit_t",
                "kind": "TypeAlias",
                "addr": 100,
                "target": "struct packed{logic[3:0] a;}cfg_pkg::s$1"
            }],
            "design": { "members": [{
                "kind": "Instance",
                "name": "child",
                "body": {
                    "name": "child",
                    "kind": "InstanceBody",
                    "members": [
                        { "name": "T", "kind": "TypeParameter", "type": "100 cfg_pkg::flit_t" },
                        { "name": "U", "kind": "TypeParameter", "type": "logic[7:0]" },
                        { "name": "a", "kind": "Port", "direction": "In",
                          "type": "999 child.T[2:0]",
                          "source_file": "c.sv", "source_line": 3, "source_column": 5 },
                        { "name": "b", "kind": "Port", "direction": "Out",
                          "type": "998 child.U[2:0]$[1:0]" }
                    ]
                }
            }]}
        });
        let resolver = TypeResolver::new(&ast);
        let params = resolver.type_parameters(&ast["design"]["members"][0]["body"]);
        assert_eq!(
            resolver.resolve_with("999 child.T[2:0]", &params),
            "struct packed{logic[3:0] a;}cfg_pkg::flit_t[2:0]"
        );
        assert_eq!(
            resolver.resolve_with("998 child.U[2:0]$[1:0]", &params),
            "logic[2:0][7:0]$[1:0]"
        );
        // Without the scope's parameters the link is preserved verbatim.
        assert_eq!(resolver.resolve("999 child.T[2:0]"), "999 child.T[2:0]");

        let ports = extract_ports_from_value(&ast, false);
        assert_eq!(
            ports["child"][0].loc,
            Some(SourceLoc {
                file: "c.sv".to_string(),
                line: 3,
                column: Some(5)
            })
        );
        assert_eq!(ports["child"][1].loc, None);
    }

    #[test]
    fn resolves_slang_v11_unsized_enum_values() {
        let ast = json!({
            "nodes": [{
                "name": "offset_t",
                "kind": "EnumType",
                "addr": 100,
                "members": [
                    { "name": "ZERO", "kind": "EnumValue", "value": "0" },
                    { "name": "NEGATIVE", "kind": "EnumValue", "value": "-1" },
                    { "name": "NEGATIVE_SIZED", "kind": "EnumValue", "value": "-16'sd1" }
                ]
            }]
        });
        let resolver = TypeResolver::new(&ast);

        assert_eq!(
            resolver.resolve("100 offset_t"),
            "enum{ZERO=32'sd0,NEGATIVE=-32'sd1,NEGATIVE_SIZED=-16'sd1}offset_t"
        );
    }
}
