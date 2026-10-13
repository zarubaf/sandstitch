// SPDX-License-Identifier: Apache-2.0

//! A model of a SystemVerilog design: packages, modules, their types and
//! ports.

/// A design: packages, then modules; `modules[0]` is the top, named `top`,
/// and a module instantiates only modules after it.
#[derive(Clone)]
pub struct Design {
    pub packages: Vec<Package>,
    pub modules: Vec<Module>,
}

/// A design shows as its source, which is what a failing case reports.
impl std::fmt::Debug for Design {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\n{}", crate::print::sv(self))
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    /// `localparam int` declarations, before the typedefs; each refers only
    /// to earlier ones.
    pub params: Vec<PackageParam>,
    /// A typedef may refer to earlier typedefs of this package and to those
    /// of earlier packages.
    pub typedefs: Vec<Typedef>,
}

/// `localparam int NAME = value;`
#[derive(Clone, Debug)]
pub struct PackageParam {
    pub name: String,
    pub value: PackageValue,
}

/// The value of a package parameter.
#[derive(Clone, Copy, Debug)]
pub enum PackageValue {
    Const(i32),
    /// `package::param + offset`, an earlier parameter of this or an earlier
    /// package.
    Sum {
        package: usize,
        param: usize,
        offset: i32,
    },
}

#[derive(Clone, Debug)]
pub struct Module {
    pub name: String,
    /// Ports declared in the header (`input T x`) rather than in the body.
    /// Only when no port type uses a typedef of the module, which the
    /// header cannot see.
    pub ansi: bool,
    pub params: Vec<Param>,
    pub type_params: Vec<TypeParam>,
    /// `parameter T NAME = T'('0)` in the header, after the type parameters.
    pub typed_params: Vec<TypedParam>,
    /// Declared in the body, after the header.
    pub local_params: Vec<LocalParam>,
    pub local_type_params: Vec<LocalTypeParam>,
    pub typedefs: Vec<Typedef>,
    pub ports: Vec<PortDecl>,
    pub instances: Vec<Instance>,
}

/// `parameter int NAME = default`.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub default: i32,
}

/// `parameter type NAME = default`; the default is written in the module,
/// after the value parameters, and refers to no type parameter.
#[derive(Clone, Debug)]
pub struct TypeParam {
    pub name: String,
    pub default: DataType,
}

/// `parameter T NAME = T'('0)`, where `T` is the packed type parameter
/// `type_param` of the module.
#[derive(Clone, Debug)]
pub struct TypedParam {
    pub name: String,
    /// Index into the module's `type_params`.
    pub type_param: usize,
}

/// `localparam logic [W-1:0] NAME = '0;`, where `W` is the value parameter
/// `param` of the module.
#[derive(Clone, Debug)]
pub struct LocalParam {
    pub name: String,
    /// Index into the module's `params`.
    pub param: usize,
}

/// `localparam type NAME = T;`, where `T` is the type parameter `type_param`
/// of the module.
#[derive(Clone, Debug)]
pub struct LocalTypeParam {
    pub name: String,
    /// Index into the module's `type_params`.
    pub type_param: usize,
}

/// `typedef ty name unpacked;`
#[derive(Clone, Debug)]
pub struct Typedef {
    pub name: String,
    pub ty: DataType,
    pub unpacked: Vec<Dim>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Input,
    Output,
    InOut,
}

/// `dir ty name unpacked`.
#[derive(Clone, Debug)]
pub struct PortDecl {
    pub dir: Dir,
    pub name: String,
    pub ty: DataType,
    pub unpacked: Vec<Dim>,
}

/// `module #(.P(v), .T(type)) name array (...)`, every port connected.
#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    /// Index into [`Design::modules`].
    pub module: usize,
    /// Instance array dimensions, outermost first; constant.
    pub array: Vec<Dim>,
    /// The generate loop the instance is in, if any.
    pub generate: Option<GenerateLoop>,
    /// Overridden value parameters: index into the child's `params`, value.
    pub params: Vec<(usize, i32)>,
    /// Overridden type parameters: index into the child's `type_params`,
    /// type written in the instantiating module.
    pub type_params: Vec<(usize, DataType)>,
    /// `defparam name[k]...[l].P = v;` after the instance: one element of an
    /// instance array gets a value of its own for a parameter the instance
    /// does not override.
    pub element_param: Option<ElementParam>,
}

/// A `defparam` on one element of an instance array.
#[derive(Clone, Debug)]
pub struct ElementParam {
    /// The element's index in each dimension of the array, outermost first.
    pub element: Vec<i32>,
    /// Index into the child's `params`.
    pub param: usize,
    pub value: i32,
}

/// `for (genvar i = start; i < start + count; i++) begin : name`, or
/// descending, `for (genvar i = start; i > start - count; i--)`, around one
/// instance.
#[derive(Clone, Debug)]
pub struct GenerateLoop {
    pub name: String,
    pub start: i32,
    pub count: u32,
    pub descending: bool,
}

impl GenerateLoop {
    /// The genvar's values, in iteration order.
    pub fn values(&self) -> Vec<i32> {
        (0..self.count as i32)
            .map(|k| {
                if self.descending {
                    self.start - k
                } else {
                    self.start + k
                }
            })
            .collect()
    }
}

/// A data type as written: a base and its packed dimensions, outermost
/// first.
#[derive(Clone, Debug)]
pub struct DataType {
    pub base: Base,
    pub packed: Vec<Dim>,
}

#[derive(Clone, Debug)]
pub enum Base {
    /// `logic`, `reg` or `bit`, optionally `signed`.
    Vector {
        kw: VectorKw,
        signed: bool,
    },
    /// `int`, `byte`, ... with an optional `signed`/`unsigned`. Takes no
    /// packed dimensions.
    Integer {
        kind: IntKind,
        signed: Option<bool>,
    },
    /// An anonymous `struct` or `union` written in place.
    Aggregate(Aggregate),
    /// An anonymous `enum` written in place.
    Enum(Enum),
    Named(TypeRef),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorKw {
    Logic,
    Reg,
    Bit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntKind {
    Byte,
    ShortInt,
    Int,
    LongInt,
    Integer,
}

impl IntKind {
    pub fn keyword(self) -> &'static str {
        match self {
            IntKind::Byte => "byte",
            IntKind::ShortInt => "shortint",
            IntKind::Int => "int",
            IntKind::LongInt => "longint",
            IntKind::Integer => "integer",
        }
    }
}

/// An anonymous struct or union. `id` identifies the declaration within the
/// design, so that the oracle can tell two anonymous types apart.
#[derive(Clone, Debug)]
pub struct Aggregate {
    pub id: usize,
    pub union: bool,
    pub packed: bool,
    /// `packed signed`; only with `packed`.
    pub signed: bool,
    pub members: Vec<Member>,
}

/// `ty name unpacked;` in a struct or union.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub ty: DataType,
    pub unpacked: Vec<Dim>,
}

/// An anonymous enum; `id` as for [`Aggregate`].
#[derive(Clone, Debug)]
pub struct Enum {
    pub id: usize,
    /// `None` is the implicit base, `int`.
    pub base: Option<EnumBase>,
    pub variants: Vec<EnumVariant>,
}

#[derive(Clone, Debug)]
pub enum EnumBase {
    /// `logic signed [msb:lsb]`; `range` `None` is a single bit.
    Vector {
        kw: VectorKw,
        signed: bool,
        range: Option<(i32, i32)>,
    },
    Integer {
        kind: IntKind,
        signed: Option<bool>,
    },
}

/// `NAME` or `NAME = value`.
#[derive(Clone, Debug)]
pub struct EnumVariant {
    pub name: String,
    pub value: Option<i128>,
}

/// A reference to a named type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeRef {
    Package {
        package: usize,
        typedef: usize,
    },
    /// A typedef of the module the reference is written in.
    Local(usize),
    /// A type parameter of the module the reference is written in.
    TypeParam(usize),
}

/// `[msb:lsb]`, or `[size]` (unpacked only).
#[derive(Clone, Debug)]
pub enum Dim {
    Range(Expr, Expr),
    Size(u32),
}

impl Dim {
    /// The bounds of a constant instance array dimension, `[a:b]`.
    pub fn bounds(&self) -> (i32, i32) {
        match self {
            Dim::Range(Expr::Const(a), Expr::Const(b)) => (*a, *b),
            Dim::Size(n) => (0, *n as i32 - 1),
            other => panic!("instance array dimension {other:?} is not constant"),
        }
    }

    /// The indices of a constant instance array dimension, in ascending
    /// order whichever way the range runs.
    pub fn indices(&self) -> Vec<i32> {
        let (a, b) = self.bounds();
        (a.min(b)..=a.max(b)).collect()
    }
}

/// A constant, or a value parameter of the enclosing module plus an offset.
#[derive(Clone, Copy, Debug)]
pub enum Expr {
    Const(i32),
    Param { param: usize, offset: i32 },
}

/// The width of an integer type.
pub fn int_width(kind: IntKind) -> u64 {
    match kind {
        IntKind::Byte => 8,
        IntKind::ShortInt => 16,
        IntKind::Int | IntKind::Integer => 32,
        IntKind::LongInt => 64,
    }
}

/// The width and signedness of an enum's base type.
pub fn enum_base_width(base: Option<&EnumBase>) -> (u32, bool) {
    match base {
        None => (32, true),
        Some(EnumBase::Vector { signed, range, .. }) => {
            (range.map_or(1, |(m, l)| m.abs_diff(l) + 1), *signed)
        }
        Some(EnumBase::Integer { kind, signed }) => (
            int_width(*kind) as u32,
            // Every integer type is signed unless declared `unsigned`.
            signed.unwrap_or(true),
        ),
    }
}
