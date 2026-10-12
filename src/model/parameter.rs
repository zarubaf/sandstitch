// SPDX-License-Identifier: Apache-2.0

use num_bigint::{BigInt, BigUint};

// Represents the type of a parameter
#[derive(Clone, Debug)]
pub enum ParameterType {
    Signed(usize),
    Unsigned(usize),
}

impl ParameterType {
    pub fn width(&self) -> usize {
        match self {
            ParameterType::Signed(width) => *width,
            ParameterType::Unsigned(width) => *width,
        }
    }

    pub fn signed(&self) -> bool {
        match self {
            ParameterType::Signed(_) => true,
            ParameterType::Unsigned(_) => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ParameterSpec {
    Integer {
        value: BigInt,
        ty: ParameterType,
    },
    String(String),
    /// A type parameter override naming a type: `ident` or `pkg::ident`.
    Type(String),
    /// A value parameter override naming a constant: `ident` or `pkg::ident`.
    Expr(String),
}

impl ParameterSpec {
    pub(crate) fn value(&self) -> ParameterValue {
        match self {
            Self::Integer { value, .. } => ParameterValue::Integer(value.clone()),
            Self::String(value) => ParameterValue::String(value.clone()),
            Self::Type(name) => ParameterValue::Type(name.clone()),
            Self::Expr(name) => ParameterValue::Expr(name.clone()),
        }
    }
}

/// A parameter override supplied to
/// [`ModDef::parameterize`](crate::ModDef::parameterize) or
/// [`ModDef::try_parameterize`](crate::ModDef::try_parameterize).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParameterValue {
    /// An arbitrary-precision integer parameter value.
    Integer(BigInt),
    /// A printable-ASCII string parameter value without quotes or backslashes.
    String(String),
    /// A type for a `parameter type`, named `ident` or `pkg::ident` (for
    /// example `cfg_pkg::flit_t`). The type is resolved by Slang, so its
    /// package must be among the module's sources.
    Type(String),
    /// A named constant for a value parameter, `ident` or `pkg::ident` (for
    /// example `cfg_pkg::Depth`), resolved by Slang like [`Self::Type`].
    Expr(String),
}

macro_rules! impl_parameter_value {
    ($variant:ident: $($type:ty),+ $(,)?) => {
        $(
            impl From<$type> for ParameterValue {
                fn from(value: $type) -> Self {
                    Self::$variant(value.into())
                }
            }
        )+
    };
}

impl_parameter_value!(Integer:
    bool, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, BigInt, BigUint,
);
impl_parameter_value!(String: String, &str, &String);
