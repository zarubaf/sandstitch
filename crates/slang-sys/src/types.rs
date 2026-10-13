// SPDX-License-Identifier: Apache-2.0

//! The data a design import reports: ports, their types, parameter
//! declarations and packages.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Index;
use std::rc::Rc;
use std::str::FromStr;

use num_bigint::BigInt;

/// A SystemVerilog data type with its dimensions.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Logic {
        signed: bool,
        packed_dimensions: Vec<Range>,
        unpacked_dimensions: Vec<Range>,
    },
    Struct {
        name: String,
        /// `struct packed signed`.
        signed: bool,
        /// `struct packed`; an unpacked struct has no bit layout of its own.
        packed: bool,
        fields: Vec<Field>,
        packed_dimensions: Vec<Range>,
        unpacked_dimensions: Vec<Range>,
    },
    Union {
        name: String,
        /// `union packed signed`.
        signed: bool,
        /// `union packed`; an unpacked union has no bit layout of its own.
        packed: bool,
        fields: Vec<Field>,
        packed_dimensions: Vec<Range>,
        unpacked_dimensions: Vec<Range>,
    },
    Enum {
        name: String,
        variants: Vec<Variant>,
        packed_dimensions: Vec<Range>,
        unpacked_dimensions: Vec<Range>,
    },
}

/// A member of a struct or union.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

/// A dimension, `[msb:lsb]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    pub msb: i32,
    pub lsb: i32,
}

/// A member of an enum.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: String,
    pub width: usize,
    pub value: BigInt,
    /// Whether the value is a signed literal (`'sd`), i.e. the enum's base
    /// type is signed.
    pub signed: bool,
}

impl Type {
    fn unpacked_dimensions(&self) -> &Vec<Range> {
        match self {
            Type::Logic {
                unpacked_dimensions,
                ..
            } => unpacked_dimensions,
            Type::Struct {
                unpacked_dimensions,
                ..
            } => unpacked_dimensions,
            Type::Union {
                unpacked_dimensions,
                ..
            } => unpacked_dimensions,
            Type::Enum {
                unpacked_dimensions,
                ..
            } => unpacked_dimensions,
        }
    }

    fn packed_dimensions(&self) -> &Vec<Range> {
        match self {
            Type::Logic {
                packed_dimensions, ..
            } => packed_dimensions,
            Type::Struct {
                packed_dimensions, ..
            } => packed_dimensions,
            Type::Enum {
                packed_dimensions, ..
            } => packed_dimensions,
            Type::Union {
                packed_dimensions, ..
            } => packed_dimensions,
        }
    }

    fn number_of_elements(&self) -> Result<usize, &'static str> {
        if !self.unpacked_dimensions().is_empty() {
            return Err("Unpacked dimensions are not supported in width calculations");
        }

        Ok(self
            .packed_dimensions()
            .iter()
            .map(|Range { msb, lsb }| ((msb - lsb).abs() + 1) as usize)
            .product())
    }

    /// The width in bits; an error for a type without a bit layout (unpacked
    /// dimensions, an empty enum, or an enum with different-sized variants).
    pub fn width(&self) -> Result<usize, &'static str> {
        match self {
            Type::Logic { .. } => self.number_of_elements(),
            Type::Struct { fields, .. } => {
                let mut width = 0;
                for field in fields {
                    width += field.ty.width()?;
                }
                Ok(width * self.number_of_elements()?)
            }
            Type::Union { fields, .. } => {
                let mut width = 0;
                for field in fields {
                    width = width.max(field.ty.width()?);
                }
                Ok(width * self.number_of_elements()?)
            }
            Type::Enum { variants, .. } => {
                if variants.is_empty() {
                    return Err("Enum with no variants");
                }

                let width = variants[0].width;
                for variant in variants[1..].iter() {
                    if width != variant.width {
                        return Err("Enum with different-sized variants");
                    }
                }
                Ok(width * self.number_of_elements()?)
            }
        }
    }
}

/// The direction of a port.
#[derive(Clone, Debug, PartialEq)]
pub enum PortDir {
    Input,
    Output,
    InOut,
}

impl FromStr for PortDir {
    type Err = String;

    /// Parses Slang's direction names: `In`, `Out` and `InOut`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "In" => Ok(PortDir::Input),
            "Out" => Ok(PortDir::Output),
            "InOut" => Ok(PortDir::InOut),
            _ => Err(format!("Unsupported I/O direction: {}", s)),
        }
    }
}

/// A declaration's position in the source.
///
/// `file` is the source path. `line` is 1-based. `column` is 1-based, or
/// `None` when the source has no column (column 0), as for declarations
/// produced by a macro expansion, where `line` is the line of the macro use.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourceLoc {
    pub file: String,
    pub line: u32,
    pub column: Option<u32>,
}

/// A port of a module with a data type.
#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub dir: PortDir,
    pub name: String,
    pub ty: Type,
    /// Where the port is declared, if source information is available.
    pub loc: Option<SourceLoc>,
}

/// A port whose type is an interface (`bus_if.mst m_if` or `bus_if s_if`).
///
/// Interface-array dimensions (`bus_if m_if [2]`) are not reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfacePort {
    pub name: String,
    /// The interface definition name; empty for generic (`interface`) ports.
    pub interface: String,
    pub modport: Option<String>,
    pub loc: Option<SourceLoc>,
}

/// A port that is not a [`Port`] or an [`InterfacePort`] (for example a
/// `real`, `string` or `event` port, or a `ref` port).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedPort {
    pub name: String,
    /// Slang's display of the port's type, with type-parameter aliases seen
    /// through (`real` for a port of type `T = real`, `real$[0:1]` for
    /// `T r [2]`); typedef names stay as written (`e_t`, `u_t$[0:1]`).
    pub type_text: String,
    /// Why the port is not supported.
    pub error: String,
    pub loc: Option<SourceLoc>,
}

/// A net or variable declared in a module body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub name: String,
    /// Slang's display of the type, with type-parameter aliases seen
    /// through; typedef names stay as written (`p::s_t`, `logic[3:0]`).
    pub type_text: String,
    /// The width in bits (Slang's bitstream width, `$bits`); 0 for a type
    /// without one.
    pub width: usize,
}

/// A parameter declaration with its data type.
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterDef {
    pub name: String,
    pub ty: Type,
}

/// Whether a parameter takes a value or a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterKind {
    Value,
    Type,
}

/// A package member: a value parameter or a type alias.
#[derive(Clone, Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    /// The constant as Slang prints it, or for an alias the alias as Slang's
    /// type printer expands it with its package-qualified name
    /// (`struct packed{logic[1:0] a;}my_pkg::my_struct_t`, `logic[33:22]`).
    pub value: String,
    /// The aliased type, set for an alias whose type is supported; its
    /// structs, unions and enums are named as a port of that type would be.
    pub ty: Option<Type>,
}

impl Parameter {
    /// Parse the parameter's `value` into any type that implements [`FromStr`].
    ///
    /// # Type Parameters
    ///
    /// * **`T`** – The numeric type you want (`i64`, `u128`,
    ///   [`num_bigint::BigInt`], [`num_bigint::BigUint`], `f64`, ...).
    ///   `T` only needs to satisfy `T: FromStr`.
    ///
    /// # Errors
    ///
    /// Returns `Err(T::Err)` if `value` is *not* a valid textual
    /// representation for `T`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use num_bigint::{BigInt, BigUint};
    ///
    /// # use slang_sys::Parameter;
    /// let p = Parameter { name: "answer".into(), value: "42".into(), ty: None };
    ///
    /// // Primitive integer (type inferred)
    /// let n: i32 = p.parse().unwrap();
    ///
    /// // Explicit turbofish when inference can’t decide
    /// let n128 = p.parse::<u128>().unwrap();
    ///
    /// // Big integers
    /// let big:  BigInt  = p.parse().unwrap();
    /// let ubig: BigUint = p.parse().unwrap();
    /// ```
    pub fn parse<T>(&self) -> Result<T, T::Err>
    where
        T: FromStr,
    {
        self.value.parse()
    }
}

/// A package with its value parameters and type aliases by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Package {
    pub name: String,
    pub parameters: HashMap<String, Parameter>,
}

impl Index<&str> for Package {
    type Output = Parameter;

    fn index(&self, key: &str) -> &Self::Output {
        &self.parameters[key]
    }
}

/// An instance in the hierarchy of a design.
#[derive(Debug, PartialEq)]
pub struct Instance {
    /// The definition's name.
    pub def_name: String,
    /// The instance's name; for an instance array element `u[3]`.
    pub inst_name: String,
    /// The generate blocks the instance is in: empty outside them, otherwise
    /// a leading dot and the block names dot-joined, loop elements named by
    /// the genvar's value (`.g[2].h`).
    pub hier_prefix: String,
    pub contents: Vec<Rc<RefCell<Instance>>>,
}
