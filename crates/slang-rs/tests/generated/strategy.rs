// SPDX-License-Identifier: Apache-2.0

//! The strategy that generates valid models.
//!
//! Generation has two stages. Proptest generates a raw tree in which type
//! references, parameter references and instance targets are free numbers;
//! `lower` turns it into a [`Design`] that is valid by construction: every
//! reference picks among the declarations visible and suitable at that
//! point, packed contexts get packed types, packed unions get members of one
//! width, `inout` ports get net types, and every name is unique.

use proptest::prelude::*;

use crate::model::{
    Aggregate, Base, DataType, Design, Dim, Dir, Enum, EnumBase, EnumVariant, Expr, Instance,
    IntKind, LocalParam, LocalTypeParam, Member, Module, Package, Param, PortDecl, TypeParam,
    TypeRef, Typedef, VectorKw, enum_base_width, int_width,
};

/// Instances nest at most this deep, the top included.
const MAX_DEPTH: usize = 3;
const MAX_PORTS: usize = 6;
/// Value parameters take values in this range, default or overridden.
const PARAM_VALUES: (i32, i32) = (1, 6);
/// Packed types are at most this wide; Slang allows 2^24 - 1 bits.
const MAX_PACKED_BITS: u64 = 1 << 12;
/// A packed type parameter's value is at most this wide.
const TYPE_PARAM_BITS: u64 = 64;

/// The strategy for valid designs.
pub fn design() -> impl Strategy<Value = Design> {
    raw_design().prop_map(lower)
}

// ---------------------------------------------------------------------------
// Raw trees.

#[derive(Clone, Debug)]
struct RawDim {
    size_form: bool,
    msb: i32,
    lsb: i32,
    size: u32,
    msb_param: Option<(usize, i32)>,
    lsb_param: Option<(usize, i32)>,
}

#[derive(Clone, Debug)]
enum RawType {
    Vector {
        kw: VectorKw,
        signed: bool,
        dims: Vec<RawDim>,
    },
    Integer {
        kind: IntKind,
        signed: Option<bool>,
    },
    Enum(RawEnum, Vec<RawDim>),
    Named {
        pick: usize,
        local: bool,
        dims: Vec<RawDim>,
    },
    Aggregate {
        union: bool,
        packed: bool,
        signed: bool,
        members: Vec<(RawType, Vec<RawDim>)>,
        dims: Vec<RawDim>,
    },
}

#[derive(Clone, Debug)]
struct RawEnum {
    base: RawEnumBase,
    seed: u128,
    offsets: Vec<u8>,
    implicit: Vec<bool>,
}

#[derive(Clone, Debug)]
enum RawEnumBase {
    Implicit,
    Vector {
        kw: VectorKw,
        signed: bool,
        width: u32,
        ascending: bool,
        lsb: i32,
        scalar: bool,
    },
    Integer {
        kind: IntKind,
        signed: Option<bool>,
    },
}

#[derive(Clone, Debug)]
struct RawTypedef {
    ty: RawType,
    unpacked: Vec<RawDim>,
    /// In a module with type parameters: `typedef T tt;` for one of them
    /// instead of `ty`.
    type_parameter: Option<usize>,
}

#[derive(Clone, Debug)]
struct RawPort {
    dir: Dir,
    ty: RawType,
    unpacked: Vec<RawDim>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeClass {
    /// Any type; used only where an unpacked type is allowed.
    Any,
    /// A packed type; may take packed dimensions.
    Packed,
    /// A packed 4-state type; may also type an `inout` port.
    PackedNet,
}

#[derive(Clone, Debug)]
struct RawTypeParam {
    class: TypeClass,
    default: RawType,
}

#[derive(Clone, Debug)]
struct RawInstance {
    target: usize,
    array: Vec<RawDim>,
    params: Vec<Option<i32>>,
    type_params: Vec<Option<RawType>>,
}

#[derive(Clone, Debug)]
struct RawModule {
    ansi: bool,
    params: Vec<i32>,
    type_params: Vec<RawTypeParam>,
    typedefs: Vec<RawTypedef>,
    ports: Vec<RawPort>,
    instances: Vec<RawInstance>,
}

#[derive(Clone, Debug)]
struct RawDesign {
    packages: Vec<Vec<RawTypedef>>,
    modules: Vec<RawModule>,
}

fn raw_dim() -> impl Strategy<Value = RawDim> {
    let param = prop::option::weighted(0.4, (0usize..4, -2i32..=2));
    (
        prop::bool::weighted(0.3),
        -3i32..=4,
        -3i32..=4,
        1u32..=3,
        param.clone(),
        param,
    )
        .prop_map(|(size_form, msb, lsb, size, msb_param, lsb_param)| RawDim {
            size_form,
            msb,
            lsb,
            size,
            msb_param,
            lsb_param,
        })
}

fn dims(max: usize) -> impl Strategy<Value = Vec<RawDim>> {
    prop::collection::vec(raw_dim(), 0..=max)
}

fn vector_kw() -> impl Strategy<Value = VectorKw> {
    prop_oneof![
        3 => Just(VectorKw::Logic),
        1 => Just(VectorKw::Reg),
        2 => Just(VectorKw::Bit),
    ]
}

fn int_kind() -> impl Strategy<Value = IntKind> {
    prop_oneof![
        Just(IntKind::Byte),
        Just(IntKind::ShortInt),
        Just(IntKind::Int),
        Just(IntKind::LongInt),
        Just(IntKind::Integer),
    ]
}

fn raw_enum() -> impl Strategy<Value = RawEnum> {
    let base = prop_oneof![
        1 => Just(RawEnumBase::Implicit),
        3 => (
            vector_kw(),
            any::<bool>(),
            prop::sample::select(vec![1u32, 2, 3, 5, 8, 16, 31, 32, 33, 63, 64, 65, 70]),
            any::<bool>(),
            -3i32..=3,
            prop::bool::weighted(0.1),
        )
            .prop_map(|(kw, signed, width, ascending, lsb, scalar)| {
                RawEnumBase::Vector {
                    kw,
                    signed,
                    width,
                    ascending,
                    lsb,
                    scalar,
                }
            }),
        1 => (int_kind(), prop::option::of(any::<bool>()))
            .prop_map(|(kind, signed)| RawEnumBase::Integer { kind, signed }),
    ];
    (
        base,
        any::<u128>(),
        prop::collection::vec(0u8..8, 1..=4),
        prop::collection::vec(any::<bool>(), 4),
    )
        .prop_map(|(base, seed, offsets, implicit)| RawEnum {
            base,
            seed,
            offsets,
            implicit,
        })
}

fn raw_type() -> impl Strategy<Value = RawType> {
    let leaf = prop_oneof![
        3 => (vector_kw(), any::<bool>(), dims(2))
            .prop_map(|(kw, signed, dims)| RawType::Vector { kw, signed, dims }),
        1 => (int_kind(), prop::option::of(any::<bool>()))
            .prop_map(|(kind, signed)| RawType::Integer { kind, signed }),
        2 => (raw_enum(), dims(1)).prop_map(|(e, dims)| RawType::Enum(e, dims)),
        4 => (any::<usize>(), any::<bool>(), dims(2))
            .prop_map(|(pick, local, dims)| RawType::Named { pick, local, dims }),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            prop::collection::vec((inner, dims(1)), 1..=3),
            dims(1),
        )
            .prop_map(
                |(union, packed, signed, members, dims)| RawType::Aggregate {
                    union,
                    packed,
                    signed,
                    members,
                    dims,
                },
            )
    })
}

fn raw_typedef() -> impl Strategy<Value = RawTypedef> {
    (
        raw_type(),
        dims(2),
        prop::option::weighted(0.15, any::<usize>()),
    )
        .prop_map(|(ty, unpacked, type_parameter)| RawTypedef {
            ty,
            unpacked,
            type_parameter,
        })
}

fn raw_module() -> impl Strategy<Value = RawModule> {
    let dir = prop_oneof![
        3 => Just(Dir::Input),
        2 => Just(Dir::Output),
        1 => Just(Dir::InOut),
    ];
    let port =
        (dir, raw_type(), dims(2)).prop_map(|(dir, ty, unpacked)| RawPort { dir, ty, unpacked });
    let class = prop_oneof![
        Just(TypeClass::Any),
        Just(TypeClass::Packed),
        Just(TypeClass::PackedNet),
    ];
    let type_param =
        (class, raw_type()).prop_map(|(class, default)| RawTypeParam { class, default });
    let array_dim = (-2i32..=2, -2i32..=2, 1u32..=3, prop::bool::weighted(0.3)).prop_map(
        |(msb, offset, size, size_form)| RawDim {
            size_form,
            msb,
            lsb: msb + offset,
            size,
            msb_param: None,
            lsb_param: None,
        },
    );
    let instance = (
        any::<usize>(),
        prop::collection::vec(array_dim, 0..=2),
        prop::collection::vec(prop::option::of(PARAM_VALUES.0..=PARAM_VALUES.1), 0..=2),
        prop::collection::vec(prop::option::of(raw_type()), 0..=2),
    )
        .prop_map(|(target, array, params, type_params)| RawInstance {
            target,
            array,
            params,
            type_params,
        });
    (
        any::<bool>(),
        prop::collection::vec(PARAM_VALUES.0..=PARAM_VALUES.1, 0..=2),
        prop::collection::vec(type_param, 0..=2),
        prop::collection::vec(raw_typedef(), 0..=4),
        prop::collection::vec(port, 0..=MAX_PORTS),
        prop::collection::vec(instance, 0..=3),
    )
        .prop_map(
            |(ansi, params, type_params, typedefs, ports, instances)| RawModule {
                ansi,
                params,
                type_params,
                typedefs,
                ports,
                instances,
            },
        )
}

fn raw_design() -> impl Strategy<Value = RawDesign> {
    (
        prop::collection::vec(prop::collection::vec(raw_typedef(), 0..=4), 1..=2),
        prop::collection::vec(raw_module(), 1..=4),
    )
        .prop_map(|(packages, modules)| RawDesign { packages, modules })
}

// ---------------------------------------------------------------------------
// Lowering.

/// What a context may use and what it requires of a type.
#[derive(Clone, Copy, Debug)]
struct Props {
    /// A packed type: may take packed dimensions and be a member of a
    /// packed aggregate.
    packed: bool,
    /// A valid net type (4-state); may type an `inout` port.
    net: bool,
    /// The width in bits, when packed and the same in every instance.
    width: Option<u64>,
    /// The largest width in any instance, when packed.
    bound: u64,
}

#[derive(Clone, Copy)]
struct Ctx<'a> {
    /// The named types visible here, with their properties.
    refs: &'a [(TypeRef, Props)],
    /// The number of value parameters visible here.
    params: usize,
    packed: bool,
    net: bool,
    /// The width must be known: no parameter in a dimension, no type
    /// parameter.
    const_width: bool,
    /// An anonymous struct, union or enum may be written here.
    inline: bool,
    /// An anonymous enum may be written here.
    inline_enum: bool,
}

impl Ctx<'_> {
    fn accepts(&self, props: &Props) -> bool {
        (!self.packed || props.packed)
            && (!self.net || props.net)
            && (!self.const_width || props.width.is_some())
    }
}

struct Lower {
    next_name: usize,
    next_id: usize,
}

impl Lower {
    fn name(&mut self, prefix: &str) -> String {
        self.next_name += 1;
        format!("{prefix}{}", self.next_name)
    }

    fn id(&mut self) -> usize {
        self.next_id += 1;
        self.next_id
    }

    fn expr(&self, value: i32, param: Option<(usize, i32)>, ctx: &Ctx) -> Expr {
        match param {
            Some((param, offset)) if ctx.params > 0 && !ctx.const_width => Expr::Param {
                param: param % ctx.params,
                offset,
            },
            _ => Expr::Const(value),
        }
    }

    /// A dimension; `packed` ones are always ranges. Its size when the same
    /// in every instance, and its largest size.
    fn dim(&self, raw: &RawDim, ctx: &Ctx, packed: bool) -> (Dim, Option<u64>, u64) {
        if raw.size_form && !packed {
            let size = u64::from(raw.size);
            return (Dim::Size(raw.size), Some(size), size);
        }
        let msb = self.expr(raw.msb, raw.msb_param, ctx);
        let lsb = self.expr(raw.lsb, raw.lsb_param, ctx);
        let size = match (msb, lsb) {
            (Expr::Const(m), Expr::Const(l)) => Some(u64::from(m.abs_diff(l)) + 1),
            _ => None,
        };
        let ((m_lo, m_hi), (l_lo, l_hi)) = (expr_range(msb), expr_range(lsb));
        let bound = u64::from(m_hi.abs_diff(l_lo).max(l_hi.abs_diff(m_lo))) + 1;
        (Dim::Range(msb, lsb), size, bound)
    }

    /// Packed dimensions and the factor they multiply the width by: exact
    /// when the same in every instance, and the largest.
    fn packed_dims(&self, raw: &[RawDim], ctx: &Ctx) -> (Vec<Dim>, Option<u64>, u64) {
        let mut factor = Some(1);
        let mut bound = 1;
        let dims = raw
            .iter()
            .map(|raw| {
                let (dim, size, size_bound) = self.dim(raw, ctx, true);
                factor = factor.zip(size).map(|(f, s)| f * s);
                bound *= size_bound;
                dim
            })
            .collect();
        (dims, factor, bound)
    }

    fn unpacked_dims(&self, raw: &[RawDim], ctx: &Ctx) -> Vec<Dim> {
        raw.iter().map(|raw| self.dim(raw, ctx, false).0).collect()
    }

    fn logic(&self, raw_dims: &[RawDim], ctx: &Ctx) -> (DataType, Props) {
        let (packed, factor, bound) = self.packed_dims(raw_dims, ctx);
        (
            DataType {
                base: Base::Vector {
                    kw: VectorKw::Logic,
                    signed: false,
                },
                packed,
            },
            Props {
                packed: true,
                net: true,
                width: factor,
                bound,
            },
        )
    }

    /// `raw` lowered for `ctx`; a single bit instead of a packed type wider
    /// than Slang allows.
    fn ty(&mut self, raw: &RawType, ctx: &Ctx) -> (DataType, Props) {
        let (ty, props) = self.ty_unbounded(raw, ctx);
        if props.packed && props.bound > MAX_PACKED_BITS {
            return self.logic(&[], ctx);
        }
        (ty, props)
    }

    /// The value of a type parameter: a packed one at most
    /// `TYPE_PARAM_BITS` wide.
    fn type_value(&mut self, raw: &RawType, ctx: &Ctx) -> DataType {
        let (ty, props) = self.ty(raw, ctx);
        if props.packed && props.bound > TYPE_PARAM_BITS {
            return self.logic(&[], ctx).0;
        }
        ty
    }

    fn ty_unbounded(&mut self, raw: &RawType, ctx: &Ctx) -> (DataType, Props) {
        match raw {
            RawType::Vector { kw, signed, dims } => {
                let kw = if ctx.net && *kw == VectorKw::Bit {
                    VectorKw::Logic
                } else {
                    *kw
                };
                let (packed, factor, bound) = self.packed_dims(dims, ctx);
                (
                    DataType {
                        base: Base::Vector {
                            kw,
                            signed: *signed,
                        },
                        packed,
                    },
                    Props {
                        packed: true,
                        net: kw != VectorKw::Bit,
                        width: factor,
                        bound,
                    },
                )
            }
            RawType::Integer { kind, signed } => {
                let kind = if ctx.net { IntKind::Integer } else { *kind };
                (
                    DataType {
                        base: Base::Integer {
                            kind,
                            signed: *signed,
                        },
                        packed: vec![],
                    },
                    Props {
                        packed: true,
                        net: kind == IntKind::Integer,
                        width: Some(int_width(kind)),
                        bound: int_width(kind),
                    },
                )
            }
            RawType::Enum(raw_enum, dims) => {
                if !ctx.inline || !ctx.inline_enum {
                    return self.logic(dims, ctx);
                }
                let (e, net, width) = self.enumeration(raw_enum, ctx);
                let (packed, factor, bound) = self.packed_dims(dims, ctx);
                (
                    DataType {
                        base: Base::Enum(e),
                        packed,
                    },
                    Props {
                        packed: true,
                        net,
                        width: factor.map(|f| f * width),
                        bound: bound * width,
                    },
                )
            }
            RawType::Named { pick, local, dims } => {
                let mut candidates: Vec<_> =
                    ctx.refs.iter().filter(|(_, p)| ctx.accepts(p)).collect();
                // Package typedefs outnumber a module's own; `local` picks
                // among the module's typedefs and type parameters if any.
                let module_level: Vec<_> = candidates
                    .iter()
                    .copied()
                    .filter(|(r, _)| !matches!(r, TypeRef::Package { .. }))
                    .collect();
                if *local && !module_level.is_empty() {
                    candidates = module_level;
                }
                if candidates.is_empty() {
                    return self.logic(dims, ctx);
                }
                let (target, props) = *candidates[pick % candidates.len()];
                let (packed, factor, bound) = if props.packed {
                    self.packed_dims(dims, ctx)
                } else {
                    (vec![], Some(1), 1)
                };
                (
                    DataType {
                        base: Base::Named(target),
                        packed,
                    },
                    Props {
                        width: props.width.zip(factor).map(|(w, f)| w * f),
                        bound: props.bound * bound,
                        ..props
                    },
                )
            }
            RawType::Aggregate {
                union,
                packed,
                signed,
                members,
                dims,
            } => {
                if !ctx.inline {
                    return self.logic(dims, ctx);
                }
                let packed = *packed || ctx.packed;
                let member_ctx = Ctx {
                    packed,
                    const_width: ctx.const_width || (*union && packed),
                    inline: true,
                    // TODO(#35): an anonymous enum declared in a struct or
                    // union makes Slang's JSON AST serializer fail an
                    // assertion (`Symbol::as<VariableSymbol>`), for example
                    // `module top(input struct packed { enum logic {A, B} e; } x); endmodule`.
                    // Allow it when the JSON path is removed.
                    inline_enum: false,
                    ..*ctx
                };
                let mut lowered: Vec<Member> = Vec::new();
                let mut props: Vec<Props> = Vec::new();
                for (raw_ty, raw_unpacked) in members {
                    let (mut ty, mut p) = self.ty(raw_ty, &member_ctx);
                    if *union && packed {
                        let width = p.width.expect("a constant width");
                        if let Some(first) = props.first().and_then(|p| p.width) {
                            if width != first {
                                ty = vector(first);
                                p = Props {
                                    packed: true,
                                    net: true,
                                    width: Some(first),
                                    bound: first,
                                };
                            }
                        }
                    }
                    let unpacked = if packed {
                        vec![]
                    } else {
                        self.unpacked_dims(raw_unpacked, &member_ctx)
                    };
                    if !unpacked.is_empty() {
                        p.packed = false;
                        p.width = None;
                    }
                    lowered.push(Member {
                        name: self.name("f"),
                        ty,
                        unpacked,
                    });
                    props.push(p);
                }
                let (width, bound) = if !packed {
                    (None, 0)
                } else if *union {
                    (props[0].width, props[0].bound)
                } else {
                    (
                        props.iter().map(|p| p.width).sum(),
                        props.iter().map(|p| p.bound).sum(),
                    )
                };
                let net = if packed {
                    props.iter().any(|p| p.net)
                } else {
                    props.iter().all(|p| p.net)
                };
                let (packed_dims, factor, factor_bound) = if packed {
                    self.packed_dims(dims, ctx)
                } else {
                    (vec![], Some(1), 1)
                };
                (
                    DataType {
                        base: Base::Aggregate(Aggregate {
                            id: self.id(),
                            union: *union,
                            packed,
                            signed: packed && *signed,
                            members: lowered,
                        }),
                        packed: packed_dims,
                    },
                    Props {
                        packed,
                        net,
                        width: width.zip(factor).map(|(w, f)| w * f),
                        bound: bound * factor_bound,
                    },
                )
            }
        }
    }

    /// An enum with distinct values within its base type's range, written
    /// implicitly where the previous value plus one gives them.
    fn enumeration(&mut self, raw: &RawEnum, ctx: &Ctx) -> (Enum, bool, u64) {
        let base = match &raw.base {
            RawEnumBase::Implicit if !ctx.net => None,
            RawEnumBase::Implicit => Some(EnumBase::Integer {
                kind: IntKind::Integer,
                signed: None,
            }),
            RawEnumBase::Vector {
                kw,
                signed,
                width,
                ascending,
                lsb,
                scalar,
            } => {
                let kw = if ctx.net && *kw == VectorKw::Bit {
                    VectorKw::Logic
                } else {
                    *kw
                };
                let range = if *scalar {
                    None
                } else {
                    let other = lsb + *width as i32 - 1;
                    Some(if *ascending {
                        (*lsb, other)
                    } else {
                        (other, *lsb)
                    })
                };
                Some(EnumBase::Vector {
                    kw,
                    signed: *signed,
                    range,
                })
            }
            RawEnumBase::Integer { kind, signed } => Some(EnumBase::Integer {
                kind: if ctx.net { IntKind::Integer } else { *kind },
                signed: *signed,
            }),
        };
        let (width, signed) = enum_base_width(base.as_ref());
        let net = match &base {
            None => false,
            Some(EnumBase::Vector { kw, .. }) => *kw != VectorKw::Bit,
            Some(EnumBase::Integer { kind, .. }) => *kind == IntKind::Integer,
        };
        // The most negative value is left out: `-W'sdN` cannot write it.
        let (lo, hi) = if signed {
            let max = (1i128 << (width - 1)) - 1;
            (-max, max)
        } else {
            (0, (1i128 << width) - 1)
        };
        let size = (hi - lo + 1) as u128;
        let start = (raw.seed % size) as i128;
        let mut values: Vec<i128> = Vec::new();
        for offset in &raw.offsets {
            let value = lo + ((start + i128::from(*offset)) as u128 % size) as i128;
            if !values.contains(&value) {
                values.push(value);
            }
        }
        let mut variants = Vec::new();
        for (i, value) in values.iter().enumerate() {
            let implied = if i == 0 { 0 } else { values[i - 1] + 1 };
            let explicit = *value != implied || !raw.implicit[i % raw.implicit.len()];
            variants.push(EnumVariant {
                name: self.name("E"),
                value: explicit.then_some(*value),
            });
        }
        (
            Enum {
                id: self.id(),
                base,
                variants,
            },
            net,
            u64::from(width),
        )
    }

    fn typedefs(
        &mut self,
        raw: &[RawTypedef],
        refs: &mut Vec<(TypeRef, Props)>,
        params: usize,
        make_ref: impl Fn(usize) -> TypeRef,
    ) -> Vec<Typedef> {
        let mut typedefs = Vec::new();
        for (i, raw) in raw.iter().enumerate() {
            let ctx = Ctx {
                refs,
                params,
                packed: false,
                net: false,
                const_width: false,
                inline: true,
                inline_enum: true,
            };
            let type_parameters: Vec<_> = refs
                .iter()
                .filter(|(r, _)| matches!(r, TypeRef::TypeParam(_)))
                .copied()
                .collect();
            let (ty, mut props, unpacked) = match raw.type_parameter {
                Some(pick) if !type_parameters.is_empty() => {
                    let (target, props) = type_parameters[pick % type_parameters.len()];
                    let ty = DataType {
                        base: Base::Named(target),
                        packed: vec![],
                    };
                    (ty, props, vec![])
                }
                _ => {
                    let (ty, props) = self.ty(&raw.ty, &ctx);
                    (ty, props, self.unpacked_dims(&raw.unpacked, &ctx))
                }
            };
            if !unpacked.is_empty() {
                props.packed = false;
                props.width = None;
            }
            typedefs.push(Typedef {
                name: self.name("t"),
                ty,
                unpacked,
            });
            refs.push((make_ref(i), props));
        }
        typedefs
    }
}

/// `logic [width-1:0]`.
fn vector(width: u64) -> DataType {
    DataType {
        base: Base::Vector {
            kw: VectorKw::Logic,
            signed: false,
        },
        packed: vec![Dim::Range(Expr::Const(width as i32 - 1), Expr::Const(0))],
    }
}

fn type_class_props(class: TypeClass) -> Props {
    Props {
        packed: class != TypeClass::Any,
        net: class == TypeClass::PackedNet,
        width: None,
        bound: TYPE_PARAM_BITS,
    }
}

/// The smallest and largest value of `expr` in any instance.
fn expr_range(expr: Expr) -> (i32, i32) {
    match expr {
        Expr::Const(value) => (value, value),
        Expr::Param { offset, .. } => (PARAM_VALUES.0 + offset, PARAM_VALUES.1 + offset),
    }
}

fn references_local(ty: &DataType) -> bool {
    match &ty.base {
        Base::Named(TypeRef::Local(_)) => true,
        Base::Aggregate(a) => a.members.iter().any(|m| references_local(&m.ty)),
        _ => false,
    }
}

fn lower(raw: RawDesign) -> Design {
    let mut lower = Lower {
        next_name: 0,
        next_id: 0,
    };
    let mut package_refs: Vec<(TypeRef, Props)> = Vec::new();
    let mut packages = Vec::new();
    for (p, raw_typedefs) in raw.packages.iter().enumerate() {
        let name = lower.name("pkg");
        let typedefs = lower.typedefs(raw_typedefs, &mut package_refs, 0, |i| TypeRef::Package {
            package: p,
            typedef: i,
        });
        packages.push(Package { name, typedefs });
    }

    let count = raw.modules.len();
    let mut depth = vec![0usize; count];
    depth[0] = 1;
    // First the interfaces (names, parameters, type parameter classes) so
    // that instances can refer to later modules.
    let mut modules: Vec<Module> = Vec::new();
    let mut classes: Vec<Vec<TypeClass>> = Vec::new();
    for (m, raw_module) in raw.modules.iter().enumerate() {
        let name = if m == 0 {
            "top".to_string()
        } else {
            lower.name("m")
        };
        let params: Vec<Param> = raw_module
            .params
            .iter()
            .map(|&default| Param {
                name: lower.name("W"),
                default,
            })
            .collect();
        let mut type_params = Vec::new();
        for raw_param in &raw_module.type_params {
            let props = type_class_props(raw_param.class);
            let ctx = Ctx {
                refs: &package_refs,
                params: params.len(),
                packed: props.packed,
                net: props.net,
                const_width: false,
                inline: true,
                inline_enum: true,
            };
            let default = lower.type_value(&raw_param.default, &ctx);
            type_params.push(TypeParam {
                name: lower.name("T"),
                default,
            });
        }
        classes.push(raw_module.type_params.iter().map(|p| p.class).collect());
        let local_params = (0..params.len().min(2))
            .map(|param| LocalParam {
                name: lower.name("L"),
                param,
            })
            .collect();
        let local_type_params = (0..type_params.len().min(2))
            .map(|type_param| LocalTypeParam {
                name: lower.name("LT"),
                type_param,
            })
            .collect();
        modules.push(Module {
            name,
            ansi: raw_module.ansi,
            params,
            type_params,
            local_params,
            local_type_params,
            typedefs: vec![],
            ports: vec![],
            instances: vec![],
        });
    }

    for (m, raw_module) in raw.modules.iter().enumerate() {
        if depth[m] == 0 {
            // Slang checks a module no one instantiates with unknown
            // parameter values and reports errors in it; instantiate every
            // module, in the top if nowhere else.
            depth[m] = 2;
            let name = lower.name("u");
            modules[0].instances.push(Instance {
                name,
                module: m,
                array: vec![],
                params: vec![],
                type_params: vec![],
            });
        }
        let mut refs = package_refs.clone();
        refs.extend(
            classes[m]
                .iter()
                .enumerate()
                .map(|(i, class)| (TypeRef::TypeParam(i), type_class_props(*class))),
        );
        let params = modules[m].params.len();
        let typedefs = lower.typedefs(&raw_module.typedefs, &mut refs, params, TypeRef::Local);

        let mut ports = Vec::new();
        for raw_port in &raw_module.ports {
            let ctx = Ctx {
                refs: &refs,
                params,
                packed: false,
                net: raw_port.dir == Dir::InOut,
                const_width: false,
                inline: true,
                inline_enum: true,
            };
            let (ty, _) = lower.ty(&raw_port.ty, &ctx);
            let unpacked = lower.unpacked_dims(&raw_port.unpacked, &ctx);
            ports.push(PortDecl {
                dir: raw_port.dir,
                name: lower.name("p"),
                ty,
                unpacked,
            });
        }

        let mut instances = Vec::new();
        if depth[m] > 0 && depth[m] < MAX_DEPTH && m + 1 < count {
            for raw_instance in &raw_module.instances {
                let target = m + 1 + raw_instance.target % (count - m - 1);
                depth[target] = depth[target].max(depth[m] + 1);
                let array_ctx = Ctx {
                    refs: &[],
                    params: 0,
                    packed: false,
                    net: false,
                    const_width: true,
                    inline: false,
                    inline_enum: false,
                };
                let array = lower.unpacked_dims(&raw_instance.array, &array_ctx);
                let param_overrides = raw_instance
                    .params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.map(|v| (i, v)))
                    .filter(|(i, _)| *i < modules[target].params.len())
                    .collect();
                let mut type_overrides = Vec::new();
                for (i, raw_ty) in raw_instance.type_params.iter().enumerate() {
                    let (Some(raw_ty), Some(class)) = (raw_ty, classes[target].get(i)) else {
                        continue;
                    };
                    let props = type_class_props(*class);
                    // Slang's parser takes no anonymous struct, union or
                    // enum as a parameter value.
                    let ctx = Ctx {
                        refs: &refs,
                        params,
                        packed: props.packed,
                        net: props.net,
                        const_width: false,
                        inline: false,
                        inline_enum: false,
                    };
                    type_overrides.push((i, lower.type_value(raw_ty, &ctx)));
                }
                instances.push(Instance {
                    name: lower.name("u"),
                    module: target,
                    array,
                    params: param_overrides,
                    type_params: type_overrides,
                });
            }
        }

        let module = &mut modules[m];
        // TODO(#35): an anonymous enum in an ANSI port list followed by a
        // port of an anonymous struct or union makes Slang's JSON AST
        // serializer fail an assertion (`Symbol::as<VariableSymbol>`), for
        // example
        // `module top (input enum {A} p3, input struct packed {logic f;} p5); endmodule`;
        // such modules declare their ports in the body until the JSON path
        // is removed.
        let enum_before_aggregate = ports
            .iter()
            .position(|p| matches!(p.ty.base, Base::Enum(_)))
            .is_some_and(|first| {
                ports[first..]
                    .iter()
                    .any(|p| matches!(p.ty.base, Base::Aggregate(_)))
            });
        module.ansi =
            module.ansi && !enum_before_aggregate && !ports.iter().any(|p| references_local(&p.ty));
        module.typedefs = typedefs;
        module.ports = ports;
        module.instances.extend(instances);
    }

    Design { packages, modules }
}
