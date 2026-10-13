// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use crate::{Package, SlangConfig, SlangError};

/// Runs Slang and returns the packages of the design, each with its value
/// parameters and type aliases.
pub fn extract_packages(cfg: &SlangConfig) -> Result<HashMap<String, Package>, SlangError> {
    Ok(crate::run_slang(cfg)?.design.packages())
}
