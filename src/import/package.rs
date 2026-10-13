// SPDX-License-Identifier: Apache-2.0

use crate::ParserConfig;
use std::collections::HashMap;
use std::error::Error;
use std::ops::Index;
use std::path::Path;
use std::str::FromStr;

pub fn extract_packages_from_verilog_file(
    verilog: &Path,
    ignore_unknown_modules: bool,
) -> Result<HashMap<String, Package>, Box<dyn Error>> {
    extract_packages_from_verilog_files(&[verilog], ignore_unknown_modules)
}

pub fn extract_packages_from_verilog_files(
    verilog: &[&Path],
    ignore_unknown_modules: bool,
) -> Result<HashMap<String, Package>, Box<dyn Error>> {
    let cfg = ParserConfig {
        sources: &verilog
            .iter()
            .map(|path| path.to_str().unwrap())
            .collect::<Vec<_>>(),
        ignore_unknown_modules,
        ..Default::default()
    };

    extract_packages_with_config(&cfg)
}

pub fn extract_packages_from_verilog(
    verilog: impl AsRef<str>,
    ignore_unknown_modules: bool,
) -> Result<HashMap<String, Package>, Box<dyn Error>> {
    let verilog = slang_rs::str2tmpfile(verilog.as_ref()).unwrap();

    let cfg = ParserConfig {
        sources: &[verilog.path().to_str().unwrap()],
        ignore_unknown_modules,
        ..Default::default()
    };

    extract_packages_with_config(&cfg)
}

pub fn extract_packages_with_config(
    cfg: &ParserConfig,
) -> Result<HashMap<String, Package>, Box<dyn Error>> {
    let pkgs = slang_rs::extract_packages(&cfg.to_slang_config())?;

    Ok(pkgs
        .into_iter()
        .map(|(name, pkg)| {
            (
                name.clone(),
                Package {
                    name: name.clone(),
                    parameters: pkg
                        .parameters
                        .into_iter()
                        .map(|(name, param)| {
                            (
                                name.clone(),
                                Parameter {
                                    name: name.clone(),
                                    value: param.value,
                                    width: param.ty.as_ref().and_then(|ty| ty.width().ok()),
                                },
                            )
                        })
                        .collect(),
                },
            )
        })
        .collect())
}

#[derive(Debug, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub value: String,
    /// The bit width of a type alias's type, when it has one.
    width: Option<usize>,
}

impl Parameter {
    /// A parameter that is not a type with a bit width.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Parameter {
            name: name.into(),
            value: value.into(),
            width: None,
        }
    }

    /// Parse the parameter's `value` into any type that implements [`FromStr`].
    ///
    /// # Type Parameters
    ///
    /// * **`T`** – The numeric type you want (`i64`, `u128`,
    ///   [`num_bigint::BigInt`], [`num_bigint::BigUint`], `f64`, ...). `T` only
    ///   needs to satisfy `T: FromStr`.
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
    /// use std::convert::TryFrom;
    ///
    /// # use topstitch::Parameter;
    /// let p = Parameter::new("answer", "42");
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

    /// The bit width of the type alias this parameter declares.
    ///
    /// # Errors
    ///
    /// Returns an error if the parameter is not a type alias, or its type has
    /// no bit width (unpacked dimensions, an empty enum, an enum with
    /// different-sized variants).
    pub fn calc_type_width(&self) -> Result<usize, Box<dyn Error>> {
        self.width
            .ok_or_else(|| format!("`{}` is not a type with a bit width", self.name).into())
    }
}

#[derive(Debug, PartialEq)]
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

impl Package {
    /// Get a parameter from the package, returning `None` if it doesn't exist.
    pub fn get(&self, key: &str) -> Option<&Parameter> {
        self.parameters.get(key)
    }
}
