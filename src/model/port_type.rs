// SPDX-License-Identifier: Apache-2.0

//! Resolved SystemVerilog port types and their structural equivalence.
//!
//! Types are converted from Slang's output at the import boundary
//! (`crate::import::typed`). Type names are kept for display only: two types
//! are equivalent when their layouts match, whatever they are called.

use num_bigint::BigInt;
use std::fmt;

/// A packed dimension `[msb:lsb]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Dim {
    pub msb: i64,
    pub lsb: i64,
}

impl Dim {
    /// Number of elements the dimension spans (at least one).
    pub fn size(&self) -> usize {
        (self.msb - self.lsb).unsigned_abs() as usize + 1
    }
}

impl fmt::Display for Dim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}:{}]", self.msb, self.lsb)
    }
}

/// A type name, optionally qualified by its package. Displays as
/// `pkg::name` or `name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QualifiedName {
    pub package: Option<String>,
    pub name: String,
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.package {
            Some(package) => write!(f, "{package}::{}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// A member of a struct or union.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PortField {
    pub name: String,
    pub ty: PortType,
}

/// The resolved type of a port.
///
/// `packed` lists the packed dimensions applied to the type, outermost first
/// (`logic [1:0][7:0]` has `[1:0]` then `[7:0]`; `flit_t [3:0]` has `[3:0]`).
/// Struct fields are listed in declaration order; the first field is the most
/// significant. Enum variant values are given as Slang reports them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PortType {
    Logic {
        signed: bool,
        packed: Vec<Dim>,
    },
    Struct {
        name: Option<QualifiedName>,
        /// `struct packed signed`: the whole struct reads as a signed vector.
        signed: bool,
        /// Declared `packed`. An unpacked struct has no bit layout of its
        /// own: its fields cannot be selected as bit ranges.
        is_packed: bool,
        fields: Vec<PortField>,
        packed: Vec<Dim>,
    },
    Union {
        name: Option<QualifiedName>,
        /// `union packed signed`.
        signed: bool,
        /// Declared `packed`.
        is_packed: bool,
        fields: Vec<PortField>,
        packed: Vec<Dim>,
    },
    Enum {
        name: Option<QualifiedName>,
        base: Box<PortType>,
        variants: Vec<(String, BigInt)>,
        packed: Vec<Dim>,
    },
}

/// The first place where two types differ.
///
/// `path` is the dotted field path from the root of the type (`hdr.dst_id`);
/// it is empty when the roots themselves differ. `left` and `right` describe
/// what each side has at that place: a type (`logic [3:0]`), a member
/// declaration (`logic [3:0] dst_id`), an enum variant (`IDLE = 0`), or
/// `nothing` when one side has no member there. When only the signing of a
/// struct or union differs, a named type is followed by `(signed)` or
/// `(unsigned)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeDifference {
    pub path: String,
    pub left: String,
    pub right: String,
}

fn elements(packed: &[Dim]) -> usize {
    packed.iter().map(Dim::size).product()
}

fn join(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.to_string()
    } else {
        format!("{path}.{field}")
    }
}

fn write_dims(f: &mut fmt::Formatter<'_>, packed: &[Dim]) -> fmt::Result {
    if !packed.is_empty() {
        f.write_str(" ")?;
        for dim in packed {
            write!(f, "{dim}")?;
        }
    }
    Ok(())
}

fn write_fields(f: &mut fmt::Formatter<'_>, fields: &[PortField]) -> fmt::Result {
    f.write_str("{")?;
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{} {};", field.ty, field.name)?;
    }
    f.write_str("}")
}

impl fmt::Display for PortType {
    /// SystemVerilog-like rendering: named types by name, anonymous ones by
    /// their layout.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortType::Logic { signed, packed } => {
                f.write_str("logic")?;
                if *signed {
                    f.write_str(" signed")?;
                }
                write_dims(f, packed)
            }
            PortType::Struct {
                name,
                signed,
                is_packed,
                fields,
                packed,
            }
            | PortType::Union {
                name,
                signed,
                is_packed,
                fields,
                packed,
            } => {
                match name {
                    Some(name) => write!(f, "{name}")?,
                    None => {
                        let keyword = if matches!(self, PortType::Struct { .. }) {
                            "struct"
                        } else {
                            "union"
                        };
                        write!(f, "{keyword} ")?;
                        if *is_packed {
                            f.write_str("packed ")?;
                        }
                        if *signed {
                            f.write_str("signed ")?;
                        }
                        write_fields(f, fields)?;
                    }
                }
                write_dims(f, packed)
            }
            PortType::Enum {
                name,
                base,
                variants,
                packed,
            } => {
                match name {
                    Some(name) => write!(f, "{name}")?,
                    None => {
                        write!(f, "enum {base} {{")?;
                        for (i, (variant, value)) in variants.iter().enumerate() {
                            if i > 0 {
                                f.write_str(", ")?;
                            }
                            write!(f, "{variant} = {value}")?;
                        }
                        f.write_str("}")?;
                    }
                }
                write_dims(f, packed)
            }
        }
    }
}

impl PortType {
    /// Width in bits of the whole type.
    pub fn width(&self) -> usize {
        match self {
            PortType::Logic { packed, .. } => elements(packed),
            PortType::Struct { fields, packed, .. } => {
                fields.iter().map(|f| f.ty.width()).sum::<usize>() * elements(packed)
            }
            PortType::Union { fields, packed, .. } => {
                fields.iter().map(|f| f.ty.width()).max().unwrap_or(0) * elements(packed)
            }
            PortType::Enum { base, packed, .. } => base.width() * elements(packed),
        }
    }

    /// Checks structural equivalence: same kind, packedness of structs and
    /// unions, signedness (of logic vectors,
    /// structs, unions and enum bases) and packed dimensions; struct and union members with the same names in the same
    /// order and equivalent types; enums with equivalent base types and the
    /// same variant names and values. Type names are not compared.
    ///
    /// Returns the first difference found, in declaration order.
    pub fn equivalent(&self, other: &PortType) -> Result<(), TypeDifference> {
        self.equivalent_at("", other)
    }

    /// The display, with `(signed)`/`(unsigned)` after a named struct or
    /// union (an anonymous one shows its `signed` keyword already).
    fn with_signing(&self) -> String {
        match self {
            PortType::Struct {
                name: Some(_),
                signed,
                ..
            }
            | PortType::Union {
                name: Some(_),
                signed,
                ..
            } => format!("{self} ({})", if *signed { "signed" } else { "unsigned" }),
            _ => self.to_string(),
        }
    }

    /// The display, with `(packed)`/`(unpacked)` after a named struct or
    /// union (an anonymous one shows its `packed` keyword already).
    fn with_packing(&self) -> String {
        match self {
            PortType::Struct {
                name: Some(_),
                is_packed,
                ..
            }
            | PortType::Union {
                name: Some(_),
                is_packed,
                ..
            } => format!(
                "{self} ({})",
                if *is_packed { "packed" } else { "unpacked" }
            ),
            _ => self.to_string(),
        }
    }

    fn equivalent_at(&self, path: &str, other: &PortType) -> Result<(), TypeDifference> {
        let differ = || TypeDifference {
            path: path.to_string(),
            left: self.to_string(),
            right: other.to_string(),
        };
        match (self, other) {
            (
                PortType::Logic { signed, packed },
                PortType::Logic {
                    signed: other_signed,
                    packed: other_packed,
                },
            ) => {
                if signed != other_signed || packed != other_packed {
                    return Err(differ());
                }
                Ok(())
            }
            (
                PortType::Struct {
                    signed,
                    is_packed,
                    fields,
                    packed,
                    ..
                },
                PortType::Struct {
                    signed: other_signed,
                    is_packed: other_is_packed,
                    fields: other_fields,
                    packed: other_packed,
                    ..
                },
            )
            | (
                PortType::Union {
                    signed,
                    is_packed,
                    fields,
                    packed,
                    ..
                },
                PortType::Union {
                    signed: other_signed,
                    is_packed: other_is_packed,
                    fields: other_fields,
                    packed: other_packed,
                    ..
                },
            ) => {
                if is_packed != other_is_packed {
                    return Err(TypeDifference {
                        path: path.to_string(),
                        left: self.with_packing(),
                        right: other.with_packing(),
                    });
                }
                if signed != other_signed {
                    return Err(TypeDifference {
                        path: path.to_string(),
                        left: self.with_signing(),
                        right: other.with_signing(),
                    });
                }
                if packed != other_packed {
                    return Err(differ());
                }
                fields_equivalent(path, fields, other_fields)
            }
            (
                PortType::Enum {
                    base,
                    variants,
                    packed,
                    ..
                },
                PortType::Enum {
                    base: other_base,
                    variants: other_variants,
                    packed: other_packed,
                    ..
                },
            ) => {
                if packed != other_packed {
                    return Err(differ());
                }
                base.equivalent_at(path, other_base)?;
                let describe = |v: Option<&(String, BigInt)>| -> String {
                    match v {
                        Some((name, value)) => format!("{name} = {value}"),
                        None => "nothing".to_string(),
                    }
                };
                for i in 0..variants.len().max(other_variants.len()) {
                    let (left, right) = (variants.get(i), other_variants.get(i));
                    if left != right {
                        return Err(TypeDifference {
                            path: path.to_string(),
                            left: describe(left),
                            right: describe(right),
                        });
                    }
                }
                Ok(())
            }
            _ => Err(differ()),
        }
    }
}

fn fields_equivalent(
    path: &str,
    fields: &[PortField],
    other_fields: &[PortField],
) -> Result<(), TypeDifference> {
    let describe = |f: Option<&PortField>| -> String {
        match f {
            Some(field) => format!("{} {}", field.ty, field.name),
            None => "nothing".to_string(),
        }
    };
    for i in 0..fields.len().max(other_fields.len()) {
        match (fields.get(i), other_fields.get(i)) {
            (Some(left), Some(right)) if left.name == right.name => {
                left.ty.equivalent_at(&join(path, &left.name), &right.ty)?;
            }
            (left, right) => {
                let name = left.or(right).map(|f| f.name.as_str()).unwrap_or_default();
                return Err(TypeDifference {
                    path: join(path, name),
                    left: describe(left),
                    right: describe(right),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logic(signed: bool, dims: &[(i64, i64)]) -> PortType {
        PortType::Logic {
            signed,
            packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
        }
    }

    fn field(name: &str, ty: PortType) -> PortField {
        PortField {
            name: name.to_string(),
            ty,
        }
    }

    fn named(package: &str, name: &str) -> Option<QualifiedName> {
        Some(QualifiedName {
            package: Some(package.to_string()),
            name: name.to_string(),
        })
    }

    fn strukt(name: Option<QualifiedName>, fields: Vec<PortField>) -> PortType {
        PortType::Struct {
            name,
            signed: false,
            is_packed: true,
            fields,
            packed: vec![],
        }
    }

    fn enumeration(name: &str, signed: bool, variants: &[(&str, i64)]) -> PortType {
        PortType::Enum {
            name: named("p", name),
            base: Box::new(logic(signed, &[(1, 0)])),
            variants: variants
                .iter()
                .map(|&(n, v)| (n.to_string(), BigInt::from(v)))
                .collect(),
            packed: vec![],
        }
    }

    /// `hdr_t { logic [W-1:0] dst_id; logic [7:0] len; }`
    fn hdr(dst_width: i64) -> PortType {
        strukt(
            named("p", "hdr_t"),
            vec![
                field("dst_id", logic(false, &[(dst_width - 1, 0)])),
                field("len", logic(false, &[(7, 0)])),
            ],
        )
    }

    /// `flit_t { hdr_t hdr; logic [31:0] data; }`
    fn flit(name: &str, dst_width: i64) -> PortType {
        strukt(
            named("p", name),
            vec![
                field("hdr", hdr(dst_width)),
                field("data", logic(false, &[(31, 0)])),
            ],
        )
    }

    #[test]
    fn width_of_each_kind() {
        assert_eq!(logic(false, &[]).width(), 1);
        assert_eq!(logic(false, &[(1, 0), (7, 0)]).width(), 16);
        assert_eq!(logic(false, &[(0, 3)]).width(), 4);
        // 4 + 8 + 32
        assert_eq!(flit("flit_t", 4).width(), 44);
        let union = PortType::Union {
            name: None,
            signed: false,
            is_packed: true,
            fields: vec![
                field("a", logic(false, &[(3, 0)])),
                field("b", logic(false, &[(5, 0)])),
            ],
            packed: vec![Dim { msb: 2, lsb: 0 }],
        };
        assert_eq!(union.width(), 18);
        let PortType::Enum {
            name,
            base,
            variants,
            ..
        } = enumeration("e_t", false, &[("A", 0)])
        else {
            unreachable!()
        };
        let enum_array = PortType::Enum {
            name,
            base,
            variants,
            packed: vec![Dim { msb: 3, lsb: 0 }],
        };
        assert_eq!(enum_array.width(), 8);
    }

    #[test]
    fn display() {
        assert_eq!(logic(false, &[]).to_string(), "logic");
        assert_eq!(
            logic(true, &[(1, 0), (7, 0)]).to_string(),
            "logic signed [1:0][7:0]"
        );
        assert_eq!(flit("flit_t", 4).to_string(), "p::flit_t");
        let anon = strukt(
            None,
            vec![
                field("a", logic(false, &[])),
                field("b", logic(false, &[(3, 0)])),
            ],
        );
        assert_eq!(anon.to_string(), "struct packed {logic a; logic [3:0] b;}");
        let anon_enum = PortType::Enum {
            name: None,
            base: Box::new(logic(false, &[(1, 0)])),
            variants: vec![("A".into(), BigInt::from(0)), ("B".into(), BigInt::from(3))],
            packed: vec![Dim { msb: 1, lsb: 0 }],
        };
        assert_eq!(
            anon_enum.to_string(),
            "enum logic [1:0] {A = 0, B = 3} [1:0]"
        );
    }

    #[test]
    fn same_layout_different_names_is_equivalent() {
        assert_eq!(flit("flit_t", 4).equivalent(&flit("flit2_t", 4)), Ok(()));
        let anonymous = strukt(
            None,
            vec![
                field("hdr", hdr(4)),
                field("data", logic(false, &[(31, 0)])),
            ],
        );
        assert_eq!(flit("flit_t", 4).equivalent(&anonymous), Ok(()));
    }

    #[test]
    fn field_order_differs() {
        let left = strukt(
            None,
            vec![
                field("a", logic(false, &[(3, 0)])),
                field("b", logic(false, &[(3, 0)])),
            ],
        );
        let right = strukt(
            None,
            vec![
                field("b", logic(false, &[(3, 0)])),
                field("a", logic(false, &[(3, 0)])),
            ],
        );
        assert_eq!(
            left.equivalent(&right),
            Err(TypeDifference {
                path: "a".into(),
                left: "logic [3:0] a".into(),
                right: "logic [3:0] b".into(),
            })
        );
    }

    #[test]
    fn nested_field_width_differs() {
        assert_eq!(
            flit("flit_t", 4).equivalent(&flit("flit_t", 5)),
            Err(TypeDifference {
                path: "hdr.dst_id".into(),
                left: "logic [3:0]".into(),
                right: "logic [4:0]".into(),
            })
        );
    }

    #[test]
    fn missing_field() {
        let left = strukt(None, vec![field("a", logic(false, &[]))]);
        let right = strukt(
            None,
            vec![field("a", logic(false, &[])), field("b", logic(false, &[]))],
        );
        assert_eq!(
            left.equivalent(&right),
            Err(TypeDifference {
                path: "b".into(),
                left: "nothing".into(),
                right: "logic b".into(),
            })
        );
    }

    #[test]
    fn signedness_differs() {
        assert_eq!(
            logic(true, &[(7, 0)]).equivalent(&logic(false, &[(7, 0)])),
            Err(TypeDifference {
                path: "".into(),
                left: "logic signed [7:0]".into(),
                right: "logic [7:0]".into(),
            })
        );
    }

    #[test]
    fn dimension_direction_differs() {
        assert_eq!(
            logic(false, &[(3, 0)]).equivalent(&logic(false, &[(0, 3)])),
            Err(TypeDifference {
                path: "".into(),
                left: "logic [3:0]".into(),
                right: "logic [0:3]".into(),
            })
        );
    }

    #[test]
    fn kinds_differ() {
        let packed_as_logic = logic(false, &[(43, 0)]);
        assert_eq!(
            flit("flit_t", 4).equivalent(&packed_as_logic),
            Err(TypeDifference {
                path: "".into(),
                left: "p::flit_t".into(),
                right: "logic [43:0]".into(),
            })
        );
    }

    #[test]
    fn enum_variants_differ() {
        let left = enumeration("state_t", false, &[("IDLE", 0), ("BUSY", 1)]);
        assert_eq!(
            left.equivalent(&enumeration("other_t", false, &[("IDLE", 0), ("BUSY", 1)])),
            Ok(())
        );
        assert_eq!(
            left.equivalent(&enumeration("state_t", false, &[("IDLE", 0), ("BUSY", 2)])),
            Err(TypeDifference {
                path: "".into(),
                left: "BUSY = 1".into(),
                right: "BUSY = 2".into(),
            })
        );
        assert_eq!(
            left.equivalent(&enumeration("state_t", false, &[("IDLE", 0), ("WAIT", 1)])),
            Err(TypeDifference {
                path: "".into(),
                left: "BUSY = 1".into(),
                right: "WAIT = 1".into(),
            })
        );
        assert_eq!(
            left.equivalent(&enumeration("state_t", false, &[("IDLE", 0)])),
            Err(TypeDifference {
                path: "".into(),
                left: "BUSY = 1".into(),
                right: "nothing".into(),
            })
        );
    }

    #[test]
    fn enum_base_differs() {
        let unsigned = enumeration("e_t", false, &[("A", 0)]);
        let signed = enumeration("e_t", true, &[("A", 0)]);
        assert_eq!(
            unsigned.equivalent(&signed),
            Err(TypeDifference {
                path: "".into(),
                left: "logic [1:0]".into(),
                right: "logic signed [1:0]".into(),
            })
        );
    }

    #[test]
    fn enum_inside_struct_reports_field_path() {
        let left = strukt(
            None,
            vec![field("st", enumeration("e_t", false, &[("A", 0)]))],
        );
        let right = strukt(
            None,
            vec![field("st", enumeration("e_t", false, &[("A", 1)]))],
        );
        assert_eq!(
            left.equivalent(&right),
            Err(TypeDifference {
                path: "st".into(),
                left: "A = 0".into(),
                right: "A = 1".into(),
            })
        );
    }

    #[test]
    fn struct_signing_differs() {
        let fields = vec![field("a", logic(false, &[(3, 0)]))];
        let signing = |signed, name: Option<QualifiedName>| PortType::Struct {
            is_packed: true,
            name,
            signed,
            fields: fields.clone(),
            packed: vec![],
        };
        // A plain packed struct is unsigned.
        assert_eq!(
            signing(false, None).equivalent(&signing(false, named("p", "u_t"))),
            Ok(())
        );
        assert_eq!(
            signing(true, named("p", "s_t")).equivalent(&signing(false, named("p", "u_t"))),
            Err(TypeDifference {
                path: "".into(),
                left: "p::s_t (signed)".into(),
                right: "p::u_t (unsigned)".into(),
            })
        );
        assert_eq!(
            signing(true, None).equivalent(&signing(false, None)),
            Err(TypeDifference {
                path: "".into(),
                left: "struct packed signed {logic [3:0] a;}".into(),
                right: "struct packed {logic [3:0] a;}".into(),
            })
        );
        let outer = |inner| PortType::Struct {
            name: None,
            signed: false,
            is_packed: true,
            fields: vec![field("inner", inner)],
            packed: vec![],
        };
        assert_eq!(
            outer(signing(true, named("p", "s_t")))
                .equivalent(&outer(signing(false, named("p", "s_t")))),
            Err(TypeDifference {
                path: "inner".into(),
                left: "p::s_t (signed)".into(),
                right: "p::s_t (unsigned)".into(),
            })
        );
    }

    #[test]
    fn union_signing_differs() {
        let union = |signed| PortType::Union {
            is_packed: true,
            name: None,
            signed,
            fields: vec![
                field("a", logic(false, &[(3, 0)])),
                field("b", logic(false, &[(3, 0)])),
            ],
            packed: vec![],
        };
        assert_eq!(union(true).equivalent(&union(true)), Ok(()));
        assert_eq!(
            union(true).equivalent(&union(false)),
            Err(TypeDifference {
                path: "".into(),
                left: "union packed signed {logic [3:0] a; logic [3:0] b;}".into(),
                right: "union packed {logic [3:0] a; logic [3:0] b;}".into(),
            })
        );
    }

    #[test]
    fn struct_and_union_differ() {
        let fields = vec![field("a", logic(false, &[(3, 0)]))];
        let s = strukt(None, fields.clone());
        let u = PortType::Union {
            name: None,
            signed: false,
            is_packed: true,
            fields,
            packed: vec![],
        };
        assert_eq!(
            s.equivalent(&u),
            Err(TypeDifference {
                path: "".into(),
                left: "struct packed {logic [3:0] a;}".into(),
                right: "union packed {logic [3:0] a;}".into(),
            })
        );
    }

    #[test]
    fn array_dims_of_struct_differ() {
        let PortType::Struct {
            name,
            signed,
            fields,
            ..
        } = flit("flit_t", 4)
        else {
            unreachable!()
        };
        let array = |msb| PortType::Struct {
            is_packed: true,
            name: name.clone(),
            signed,
            fields: fields.clone(),
            packed: vec![Dim { msb, lsb: 0 }],
        };
        assert_eq!(
            array(1).equivalent(&array(2)),
            Err(TypeDifference {
                path: "".into(),
                left: "p::flit_t [1:0]".into(),
                right: "p::flit_t [2:0]".into(),
            })
        );
    }
}
