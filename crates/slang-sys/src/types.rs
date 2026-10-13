// SPDX-License-Identifier: Apache-2.0

//! The data a design import reports: ports, their types and parameter
//! declarations.

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
