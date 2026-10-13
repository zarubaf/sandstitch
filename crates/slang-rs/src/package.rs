// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use crate::{Package, SlangConfig, SlangError};

/// Runs Slang and returns the packages of the design, each with its value
/// parameters and type aliases.
pub fn extract_packages(cfg: &SlangConfig) -> Result<HashMap<String, Package>, SlangError> {
    Ok(crate::run_slang(cfg)?.design.packages())
}

/// The JSON AST's answer, which the differential comparison checks the
/// design against.
#[cfg(feature = "differential")]
pub(crate) mod json {
    use serde_json::Value;
    use std::collections::HashMap;

    use crate::extract::TypeResolver;
    use crate::{Package, Parameter};

    pub(crate) fn extract_packages_from_value(
        value: &Value,
        type_resolver: &TypeResolver,
    ) -> HashMap<String, Package> {
        let mut packages = HashMap::new();
        let units = value["design"]["members"].as_array();
        for unit in units.into_iter().flatten() {
            if unit["kind"] != "CompilationUnit" {
                continue;
            }
            for member in unit["members"].as_array().into_iter().flatten() {
                if let (Some("Package"), Some(name)) =
                    (member["kind"].as_str(), member["name"].as_str())
                {
                    packages.insert(
                        name.to_string(),
                        Package {
                            name: name.to_string(),
                            parameters: parameters(member, type_resolver),
                        },
                    );
                }
            }
        }
        packages
    }

    fn parameters(package: &Value, type_resolver: &TypeResolver) -> HashMap<String, Parameter> {
        let mut parameters = HashMap::new();
        for member in package["members"].as_array().into_iter().flatten() {
            let Some(name) = member["name"].as_str() else {
                continue;
            };
            let value = match member["kind"].as_str() {
                Some("Parameter") => member["value"].as_str().map(str::to_string),
                Some("TypeAlias") => member["target"].as_str().map(|target| {
                    type_resolver
                        .resolve_alias(member, name)
                        .unwrap_or_else(|| type_resolver.resolve(target))
                }),
                _ => None,
            };
            if let Some(value) = value {
                parameters.insert(
                    name.to_string(),
                    Parameter {
                        name: name.to_string(),
                        value,
                        ty: None,
                    },
                );
            }
        }
        parameters
    }
}
