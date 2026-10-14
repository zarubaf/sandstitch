// SPDX-License-Identifier: Apache-2.0

//! The typed design snapshot: what a [`run`](crate::run) with a
//! [`DesignQuery`] returns, converted from the flat tables of the bridge
//! (`ffi::FfiDesign`) and queried by instance path. See
//! `doc/design/slang-in-process.md`, "Step 2".

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use num_bigint::{BigInt, BigUint, Sign};

use crate::types::Instance;
use crate::types::{
    Field, InterfacePort, Package, Parameter, ParameterDef, ParameterKind, Port, PortDir, Range,
    Signal, SourceLoc, Type, UnsupportedPort, Variant,
};
use crate::{Error, absolute_path, ffi};

/// How much of the design a [`run`](crate::run) returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesignQuery {
    /// No design.
    None,
    /// Instances and their ports.
    Structure,
    /// Also the signals of each body.
    WithSignals,
}

/// The elaborated design: top-level instances, and for each instance body
/// its ports and children. Immutable; nothing of Slang is kept alive.
#[derive(Clone, Debug)]
pub struct Design {
    bodies: Vec<Body>,
    tops: Vec<Top>,
    packages: HashMap<String, Package>,
    has_signals: bool,
}

#[derive(Clone, Debug)]
struct Top {
    name: String,
    body: usize,
}

#[derive(Clone, Debug)]
struct Body {
    /// Data ports in declaration order; unsupported ones as `Err`.
    ports: Vec<Result<Port, UnsupportedPort>>,
    interface_ports: Vec<InterfacePort>,
    /// Every parameter whose type is supported, localparams included.
    parameters: Vec<ParameterDef>,
    /// The non-local parameters, whatever their types.
    overridable: Vec<(String, ParameterKind)>,
    children: Vec<Child>,
    signals: Vec<Signal>,
}

#[derive(Clone, Debug)]
struct Child {
    name: String,
    /// The definition's name.
    definition: String,
    /// Generate-block prefix; empty outside generate blocks.
    prefix: String,
    /// `None` for an instance of an uninstantiated definition.
    body: Option<usize>,
}

/// A type table entry after conversion.
#[derive(Clone)]
enum Resolved {
    Type(Type),
    Unsupported { type_text: String, error: String },
}

// `FfiType::kind`.
const LOGIC: u8 = 0;
const STRUCT: u8 = 1;
const UNION: u8 = 2;
const ENUM: u8 = 3;
const UNSUPPORTED: u8 = 4;

// `FfiPort::kind`.
const DATA_PORT: u8 = 0;
const INTERFACE_PORT: u8 = 1;
const UNSUPPORTED_PORT: u8 = 2;

// `FfiParameter::kind`.
const VALUE_PARAMETER: u8 = 0;
const TYPE_PARAMETER: u8 = 1;

// `FfiPackageParameter::kind`.
const PACKAGE_VALUE: u8 = 0;
const PACKAGE_ALIAS: u8 = 1;

impl Design {
    /// The top-level instances' names (their definitions' names), in the
    /// order Slang reports them.
    pub fn tops(&self) -> impl Iterator<Item = &str> {
        self.tops.iter().map(|t| t.name.as_str())
    }

    /// The data ports of the instance at `path`, in declaration order;
    /// unsupported ports as `Err`. Interface ports are in
    /// [`interface_ports`](Self::interface_ports). `None` when `path` names
    /// no instantiated instance.
    ///
    /// `path` is a top's name, or that followed by dot-separated instance
    /// names through children outside generate blocks; an instance array
    /// element is named as declared (`top.u[3]`). A segment cannot contain a
    /// `.`, so an instance with an escaped name that does (`\u.x `) has no
    /// path.
    pub fn ports(&self, path: &str) -> Option<&[Result<Port, UnsupportedPort>]> {
        self.body(path).map(|b| b.ports.as_slice())
    }

    /// The interface ports of the instance at `path`; see
    /// [`ports`](Self::ports).
    pub fn interface_ports(&self, path: &str) -> Option<&[InterfacePort]> {
        self.body(path).map(|b| b.interface_ports.as_slice())
    }

    /// The value parameters of the instance at `path` with their types, in
    /// declaration order, with the instance's overrides applied. Localparams
    /// are included; type parameters and parameters whose type is unsupported
    /// (not integral) are left out. `None` when `path` names no instantiated
    /// instance; see [`ports`](Self::ports) for `path`.
    pub fn parameters(&self, path: &str) -> Option<&[ParameterDef]> {
        self.body(path).map(|b| b.parameters.as_slice())
    }

    /// The non-local value and type parameters of the instance at `path`,
    /// whatever their types, in declaration order. `None` when `path` names
    /// no instantiated instance.
    pub fn overridable_parameters(&self, path: &str) -> Option<Vec<(String, ParameterKind)>> {
        self.body(path).map(|b| b.overridable.clone())
    }

    /// The names of the instantiated children of the instance at `path`
    /// that are outside generate blocks, in the order Slang lists them; each
    /// is a valid last segment of a path below `path`, except a name that
    /// contains a `.` (an escaped name such as `\u.x `), which is listed but
    /// not reachable by [`ports`](Self::ports) and the other path queries.
    /// An instance array element is named as declared (`u[3]`). Instances of
    /// undefined modules are not listed. `None` when `path` names no
    /// instantiated instance.
    pub fn child_instances(&self, path: &str) -> Option<Vec<&str>> {
        self.body(path).map(|b| {
            b.children
                .iter()
                .filter(|c| c.prefix.is_empty() && c.body.is_some())
                .map(|c| c.name.as_str())
                .collect()
        })
    }

    /// The nets and variables declared directly in the instance at `path`,
    /// in declaration order; a port appears as its internal net or variable.
    /// Declarations inside generate blocks are not included. `None` when
    /// the run did not request [`DesignQuery::WithSignals`] or `path` names
    /// no instantiated instance; see [`ports`](Self::ports) for `path`.
    pub fn signals(&self, path: &str) -> Option<&[Signal]> {
        if !self.has_signals {
            return None;
        }
        self.body(path).map(|b| b.signals.as_slice())
    }

    /// The name and absolute top-level file of each package, without cloning
    /// the packages.
    pub fn package_files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.packages
            .values()
            .map(|package| (package.name.as_str(), package.file.as_str()))
    }

    /// The packages of the compilation units by name, each with its value
    /// parameters and type aliases.
    pub fn packages(&self) -> HashMap<String, Package> {
        self.packages.clone()
    }

    /// The instance trees of the top-level instances, keyed by definition
    /// name. An instance's children are those outside and inside generate
    /// blocks, in the order Slang lists them, each with the generate blocks'
    /// prefix (`.g[2]`, `.g[2].h`; empty outside generate blocks, a leading
    /// dot otherwise, loop elements named by the genvar's value). The
    /// elements of an instance array are children named as declared (`u[3]`).
    /// An instance of an undefined module is a child without children;
    /// instances of uninstantiated generate blocks are not listed.
    pub fn hierarchy(&self) -> HashMap<String, Instance> {
        self.tops
            .iter()
            .map(|top| {
                let instance = Instance {
                    def_name: top.name.clone(),
                    inst_name: top.name.clone(),
                    hier_prefix: String::new(),
                    contents: self.contents(top.body),
                };
                (top.name.clone(), instance)
            })
            .collect()
    }

    fn contents(&self, body: usize) -> Vec<Rc<RefCell<Instance>>> {
        self.bodies[body]
            .children
            .iter()
            .map(|child| {
                let hier_prefix = if child.prefix.is_empty() {
                    String::new()
                } else {
                    format!(".{}", child.prefix)
                };
                Rc::new(RefCell::new(Instance {
                    def_name: child.definition.clone(),
                    inst_name: child.name.clone(),
                    hier_prefix,
                    contents: child.body.map(|b| self.contents(b)).unwrap_or_default(),
                }))
            })
            .collect()
    }

    fn body(&self, path: &str) -> Option<&Body> {
        let mut segments = path.split('.');
        let top = segments.next()?;
        let mut body = self.tops.iter().find(|t| t.name == top)?.body;
        for segment in segments {
            body = self.bodies[body]
                .children
                .iter()
                .find(|c| c.prefix.is_empty() && c.name == segment)?
                .body?;
        }
        Some(&self.bodies[body])
    }
}

fn bad(table: &str, index: usize, what: impl std::fmt::Display) -> Error {
    Error(format!("malformed design tables: {table}[{index}]: {what}"))
}

/// Checks that `index` names an entry of a table of `len` entries smaller
/// than `limit`.
fn check_index(
    table: &str,
    at: usize,
    what: &str,
    index: u32,
    len: usize,
    limit: Option<usize>,
) -> Result<usize, Error> {
    let index = index as usize;
    if index >= len {
        return Err(bad(
            table,
            at,
            format!("{what} index {index} is out of range ({len} entries)"),
        ));
    }
    if let Some(limit) = limit
        && index >= limit
    {
        return Err(bad(
            table,
            at,
            format!("{what} index {index} is not smaller than {limit}"),
        ));
    }
    Ok(index)
}

fn ranges(list: &[ffi::FfiRange]) -> Vec<Range> {
    list.iter()
        .map(|r| Range {
            msb: r.msb,
            lsb: r.lsb,
        })
        .collect()
}

/// The value of `bytes`, `width` bits of little-endian two's complement.
fn enum_value(bytes: &[u8], width: u32, signed: bool) -> Option<BigInt> {
    if width == 0 || bytes.len() != (width as usize).div_ceil(8) {
        return None;
    }
    let mask = (BigUint::from(1u8) << width) - 1u8;
    let value = BigUint::from_bytes_le(bytes) & &mask;
    let negative = signed && value.bit(u64::from(width) - 1);
    let value = BigInt::from_biguint(Sign::Plus, value);
    Some(if negative {
        value - (BigInt::from(1) << width)
    } else {
        value
    })
}

fn resolve_types(types: &[ffi::FfiType]) -> Result<Vec<Resolved>, Error> {
    let mut resolved: Vec<Resolved> = Vec::with_capacity(types.len());
    for (at, ty) in types.iter().enumerate() {
        let packed_dimensions = ranges(&ty.packed_dims);
        let unpacked_dimensions = ranges(&ty.unpacked_dims);
        let entry = match ty.kind {
            LOGIC => Resolved::Type(Type::Logic {
                signed: ty.is_signed,
                packed_dimensions,
                unpacked_dimensions,
            }),
            STRUCT | UNION => {
                let mut fields = Vec::with_capacity(ty.fields.len());
                for (i, field) in ty.fields.iter().enumerate() {
                    let index =
                        check_index("types", at, "field type", field.ty, types.len(), Some(at))
                            .map_err(|e| Error(format!("{} (field {i})", e.0)))?;
                    match &resolved[index] {
                        Resolved::Type(t) => fields.push(Field {
                            name: field.name.clone(),
                            ty: t.clone(),
                        }),
                        Resolved::Unsupported { .. } => {
                            return Err(bad(
                                "types",
                                at,
                                format!("field {i} refers to the unsupported type {index}"),
                            ));
                        }
                    }
                }
                let (name, signed, packed) = (ty.name.clone(), ty.is_signed, ty.packed);
                Resolved::Type(if ty.kind == STRUCT {
                    Type::Struct {
                        name,
                        signed,
                        packed,
                        fields,
                        packed_dimensions,
                        unpacked_dimensions,
                    }
                } else {
                    Type::Union {
                        name,
                        signed,
                        packed,
                        fields,
                        packed_dimensions,
                        unpacked_dimensions,
                    }
                })
            }
            ENUM => {
                let mut variants = Vec::with_capacity(ty.variants.len());
                for (i, v) in ty.variants.iter().enumerate() {
                    let value = enum_value(&v.value, v.width, v.is_signed).ok_or_else(|| {
                        bad(
                            "types",
                            at,
                            format!(
                                "variant {i} has {} value bytes for width {}",
                                v.value.len(),
                                v.width
                            ),
                        )
                    })?;
                    variants.push(Variant {
                        name: v.name.clone(),
                        width: v.width as usize,
                        value,
                        signed: v.is_signed,
                    });
                }
                Resolved::Type(Type::Enum {
                    name: ty.name.clone(),
                    variants,
                    packed_dimensions,
                    unpacked_dimensions,
                })
            }
            UNSUPPORTED => Resolved::Unsupported {
                type_text: ty.type_text.clone(),
                error: ty.error.clone(),
            },
            kind => return Err(bad("types", at, format!("unknown kind {kind}"))),
        };
        resolved.push(entry);
    }
    Ok(resolved)
}

fn loc(loc: &ffi::FfiLoc, cwd: &Path) -> Option<SourceLoc> {
    if loc.line == 0 {
        return None;
    }
    let file = if loc.file.is_empty() || loc.file.starts_with('<') {
        loc.file.clone()
    } else {
        absolute_path(cwd, &loc.file)
    };
    Some(SourceLoc {
        file,
        line: loc.line,
        column: (loc.column != 0).then_some(loc.column),
    })
}

/// Validates the tables and converts them. A malformed table is an
/// [`Error`], never a panic: every index must be in range and refer to a
/// smaller entry, so that no table can cycle.
pub(crate) fn from_ffi(tables: ffi::FfiDesign, cwd: &Path) -> Result<Design, Error> {
    let types = resolve_types(&tables.types)?;
    let body_count = tables.bodies.len();
    let mut bodies = Vec::with_capacity(body_count);
    for (at, body) in tables.bodies.iter().enumerate() {
        let mut ports = Vec::new();
        let mut interface_ports = Vec::new();
        for (i, port) in body.ports.iter().enumerate() {
            let place = || format!("port {i}");
            let location = loc(&port.loc, cwd);
            match port.kind {
                DATA_PORT => {
                    let ty = check_index(
                        "bodies",
                        at,
                        &format!("{} type", place()),
                        port.ty,
                        types.len(),
                        None,
                    )?;
                    ports.push(match &types[ty] {
                        Resolved::Type(ty) => Ok(Port {
                            dir: match port.dir {
                                0 => PortDir::Input,
                                1 => PortDir::Output,
                                2 => PortDir::InOut,
                                d => {
                                    return Err(bad(
                                        "bodies",
                                        at,
                                        format!("{} has unknown direction {d}", place()),
                                    ));
                                }
                            },
                            name: port.name.clone(),
                            ty: ty.clone(),
                            loc: location,
                        }),
                        Resolved::Unsupported { type_text, error } => Err(UnsupportedPort {
                            name: port.name.clone(),
                            type_text: type_text.clone(),
                            error: error.clone(),
                            loc: location,
                        }),
                    });
                }
                INTERFACE_PORT => interface_ports.push(InterfacePort {
                    name: port.name.clone(),
                    interface: port.interface.clone(),
                    modport: port.has_modport.then(|| port.modport.clone()),
                    loc: location,
                }),
                UNSUPPORTED_PORT => ports.push(Err(UnsupportedPort {
                    name: port.name.clone(),
                    type_text: port.type_text.clone(),
                    error: port.error.clone(),
                    loc: location,
                })),
                kind => {
                    return Err(bad(
                        "bodies",
                        at,
                        format!("{} has unknown kind {kind}", place()),
                    ));
                }
            }
        }
        let mut parameters = Vec::new();
        let mut overridable = Vec::new();
        for (i, parameter) in body.parameters.iter().enumerate() {
            let kind = match parameter.kind {
                VALUE_PARAMETER => ParameterKind::Value,
                TYPE_PARAMETER => ParameterKind::Type,
                kind => {
                    return Err(bad(
                        "bodies",
                        at,
                        format!("parameter {i} has unknown kind {kind}"),
                    ));
                }
            };
            if !parameter.is_local {
                overridable.push((parameter.name.clone(), kind));
            }
            if kind == ParameterKind::Value {
                let ty = check_index(
                    "bodies",
                    at,
                    &format!("parameter {i} type"),
                    parameter.ty,
                    types.len(),
                    None,
                )?;
                if let Resolved::Type(ty) = &types[ty] {
                    parameters.push(ParameterDef {
                        name: parameter.name.clone(),
                        ty: ty.clone(),
                    });
                }
            }
        }
        let mut children = Vec::with_capacity(body.children.len());
        for (i, child) in body.children.iter().enumerate() {
            let body = if child.has_body {
                Some(check_index(
                    "bodies",
                    at,
                    &format!("child {i} body"),
                    child.body,
                    body_count,
                    Some(at),
                )?)
            } else {
                None
            };
            children.push(Child {
                name: child.name.clone(),
                definition: child.definition.clone(),
                prefix: child.prefix.clone(),
                body,
            });
        }
        let signals = body
            .signals
            .iter()
            .map(|s| Signal {
                name: s.name.clone(),
                type_text: s.type_text.clone(),
                width: s.width as usize,
            })
            .collect();
        bodies.push(Body {
            signals,
            ports,
            interface_ports,
            parameters,
            overridable,
            children,
        });
    }
    let mut tops = Vec::with_capacity(tables.tops.len());
    for (at, top) in tables.tops.iter().enumerate() {
        let body = check_index("tops", at, "body", top.body, body_count, None)?;
        tops.push(Top {
            name: top.name.clone(),
            body,
        });
    }
    let mut packages = HashMap::with_capacity(tables.packages.len());
    for (at, package) in tables.packages.iter().enumerate() {
        let mut parameters = HashMap::with_capacity(package.parameters.len());
        for (i, parameter) in package.parameters.iter().enumerate() {
            let ty = match parameter.kind {
                PACKAGE_VALUE => None,
                PACKAGE_ALIAS => {
                    let ty = check_index(
                        "packages",
                        at,
                        &format!("parameter {i} type"),
                        parameter.ty,
                        types.len(),
                        None,
                    )?;
                    match &types[ty] {
                        Resolved::Type(ty) => Some(ty.clone()),
                        Resolved::Unsupported { .. } => None,
                    }
                }
                kind => {
                    return Err(bad(
                        "packages",
                        at,
                        format!("parameter {i} has unknown kind {kind}"),
                    ));
                }
            };
            parameters.insert(
                parameter.name.clone(),
                Parameter {
                    name: parameter.name.clone(),
                    value: parameter.value.clone(),
                    ty,
                },
            );
        }
        packages.insert(
            package.name.clone(),
            Package {
                name: package.name.clone(),
                file: if package.file.is_empty() || package.file.starts_with('<') {
                    package.file.clone()
                } else {
                    absolute_path(cwd, &package.file)
                },
                parameters,
            },
        );
    }
    Ok(Design {
        bodies,
        tops,
        packages,
        has_signals: tables.has_signals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic() -> ffi::FfiType {
        ffi::FfiType {
            kind: LOGIC,
            ..Default::default()
        }
    }

    fn port(name: &str, ty: u32) -> ffi::FfiPort {
        ffi::FfiPort {
            name: name.into(),
            kind: DATA_PORT,
            ty,
            ..Default::default()
        }
    }

    fn child(name: &str, prefix: &str, body: Option<u32>) -> ffi::FfiChild {
        ffi::FfiChild {
            name: name.into(),
            definition: "d".into(),
            prefix: prefix.into(),
            has_body: body.is_some(),
            body: body.unwrap_or(0),
        }
    }

    fn convert(tables: ffi::FfiDesign) -> Result<Design, Error> {
        from_ffi(tables, Path::new("/cwd"))
    }

    fn message(tables: ffi::FfiDesign) -> String {
        convert(tables).expect_err("malformed tables").0
    }

    #[test]
    fn rejects_type_index_out_of_range() {
        let msg = message(ffi::FfiDesign {
            types: vec![logic()],
            bodies: vec![ffi::FfiBody {
                ports: vec![port("p", 1)],
                ..Default::default()
            }],
            tops: vec![ffi::FfiTop {
                name: "t".into(),
                body: 0,
            }],
            ..Default::default()
        });
        assert!(
            msg.contains("bodies[0]") && msg.contains("type index 1"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_parameter_type_out_of_range() {
        let msg = message(ffi::FfiDesign {
            types: vec![logic()],
            bodies: vec![ffi::FfiBody {
                parameters: vec![ffi::FfiParameter {
                    name: "p".into(),
                    kind: VALUE_PARAMETER,
                    is_local: false,
                    ty: 1,
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        assert!(
            msg.contains("bodies[0]") && msg.contains("parameter 0 type index 1"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_forward_type_index() {
        let strukt = ffi::FfiType {
            kind: STRUCT,
            fields: vec![ffi::FfiField {
                name: "f".into(),
                ty: 1,
            }],
            ..Default::default()
        };
        let msg = message(ffi::FfiDesign {
            types: vec![strukt, logic()],
            ..Default::default()
        });
        assert!(msg.contains("types[0]") && msg.contains("index 1"), "{msg}");
    }

    #[test]
    fn rejects_forward_child_body() {
        let msg = message(ffi::FfiDesign {
            bodies: vec![
                ffi::FfiBody {
                    children: vec![child("u", "", Some(1))],
                    ..Default::default()
                },
                ffi::FfiBody::default(),
            ],
            ..Default::default()
        });
        assert!(
            msg.contains("bodies[0]") && msg.contains("body index 1"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_top_body_out_of_range() {
        let msg = message(ffi::FfiDesign {
            bodies: vec![ffi::FfiBody::default()],
            tops: vec![ffi::FfiTop {
                name: "t".into(),
                body: 1,
            }],
            ..Default::default()
        });
        assert!(
            msg.contains("tops[0]") && msg.contains("body index 1"),
            "{msg}"
        );
    }

    fn package(kind: u8, ty: u32) -> ffi::FfiDesign {
        ffi::FfiDesign {
            types: vec![logic()],
            packages: vec![ffi::FfiPackage {
                name: "p".into(),
                parameters: vec![ffi::FfiPackageParameter {
                    name: "m".into(),
                    kind,
                    value: "v".into(),
                    ty,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn rejects_package_alias_type_out_of_range() {
        let msg = message(package(PACKAGE_ALIAS, 1));
        assert!(
            msg.contains("packages[0]") && msg.contains("parameter 0 type index 1"),
            "{msg}"
        );
    }

    #[test]
    fn rejects_unknown_package_member_kind() {
        let msg = message(package(7, 0));
        assert!(
            msg.contains("packages[0]") && msg.contains("unknown kind 7"),
            "{msg}"
        );
    }

    #[test]
    fn package_members_carry_types_of_aliases_only() {
        // The type of a value parameter is ignored, even out of range.
        let packages = convert(package(PACKAGE_VALUE, 9)).unwrap().packages();
        assert_eq!(packages["p"]["m"].ty, None);
        assert_eq!(packages["p"]["m"].value, "v");
        let packages = convert(package(PACKAGE_ALIAS, 0)).unwrap().packages();
        assert!(matches!(packages["p"]["m"].ty, Some(Type::Logic { .. })));
        // An alias of an unsupported type has none.
        let mut tables = package(PACKAGE_ALIAS, 0);
        tables.types = vec![ffi::FfiType {
            kind: UNSUPPORTED,
            ..Default::default()
        }];
        assert_eq!(convert(tables).unwrap().packages()["p"]["m"].ty, None);
    }

    #[test]
    fn paths_follow_children() {
        let design = convert(ffi::FfiDesign {
            types: vec![logic()],
            bodies: vec![
                ffi::FfiBody {
                    ports: vec![port("leaf", 0)],
                    ..Default::default()
                },
                ffi::FfiBody {
                    children: vec![
                        child("u", "", Some(0)),
                        child("a[1]", "", Some(0)),
                        child("u", "g", Some(0)),
                        child("v", "g", Some(0)),
                        child("gone", "", None),
                    ],
                    ..Default::default()
                },
            ],
            tops: vec![ffi::FfiTop {
                name: "t".into(),
                body: 1,
            }],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(design.tops().collect::<Vec<_>>(), ["t"]);
        assert_eq!(design.ports("t").unwrap().len(), 0);
        assert_eq!(design.ports("t.u").unwrap().len(), 1);
        assert_eq!(design.ports("t.a[1]").unwrap().len(), 1);
        assert!(design.ports("t.g.u").is_none());
        assert!(design.ports("t.v").is_none());
        assert!(design.ports("t.x").is_none());
        assert!(design.ports("t.gone").is_none());
        assert!(design.interface_ports("t.u").is_some());
        assert!(design.ports("u").is_none());
    }

    #[test]
    fn enum_value_bytes() {
        assert_eq!(enum_value(&[0x0e], 4, true), Some(BigInt::from(-2)));
        assert_eq!(enum_value(&[0xfe], 4, true), Some(BigInt::from(-2)));
        assert_eq!(enum_value(&[0x0e], 4, false), Some(BigInt::from(14)));
        let wide = [0, 0, 0, 0, 0, 0, 0, 0, 0x20];
        let value = enum_value(&wide, 70, false).unwrap();
        assert_eq!(value, BigInt::from(1) << 69);
        assert!(value > BigInt::from(u64::MAX));
        // The same bits as a signed 70-bit value: bit 69 is the sign.
        assert_eq!(enum_value(&wide, 70, true), Some(BigInt::from(-1) << 69));
        assert!(enum_value(&[0], 70, false).is_none());
        assert!(enum_value(&[], 0, false).is_none());
    }
}
