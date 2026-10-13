// SPDX-License-Identifier: Apache-2.0

//! Typed queries over the top-level modules, shared by the integration tests.

#![allow(dead_code)]

use slang_rs::{
    Design, Instance, InterfacePort, ParameterDef, ParameterKind, Port, SlangConfig, SlangError,
    UnsupportedPort, run_slang,
};
use std::collections::HashMap;

/// The elaborated design of `cfg`; panics when Slang reports an error.
pub fn design(cfg: &SlangConfig) -> Design {
    run_slang(cfg).unwrap().design
}

/// The data ports of every top-level module by module name. An unsupported
/// port panics with its error unless `skip_unsupported`; interface ports
/// panic likewise (list them with [`top_interface_ports`]).
pub fn top_ports(design: &Design, skip_unsupported: bool) -> HashMap<String, Vec<Port>> {
    design
        .tops()
        .map(|top| {
            (
                top.to_string(),
                ports_at(design, top, skip_unsupported).unwrap(),
            )
        })
        .collect()
}

/// The data ports of the instance at `path`, as [`top_ports`].
pub fn ports_at(design: &Design, path: &str, skip_unsupported: bool) -> Option<Vec<Port>> {
    let ports = design.ports(path)?;
    if !skip_unsupported && !design.interface_ports(path).unwrap().is_empty() {
        panic!("Interface ports are not currently supported.");
    }
    Some(supported(ports, skip_unsupported))
}

fn supported(ports: &[Result<Port, UnsupportedPort>], skip_unsupported: bool) -> Vec<Port> {
    ports
        .iter()
        .filter_map(|port| match port {
            Ok(port) => Some(port.clone()),
            Err(_) if skip_unsupported => None,
            Err(unsupported) => panic!("{}", unsupported.error),
        })
        .collect()
}

/// The interface ports of every top-level module by module name.
pub fn top_interface_ports(design: &Design) -> HashMap<String, Vec<InterfacePort>> {
    design
        .tops()
        .map(|top| {
            (
                top.to_string(),
                design.interface_ports(top).unwrap().to_vec(),
            )
        })
        .collect()
}

/// The value parameters of every top-level module by module name.
pub fn top_parameter_defs(design: &Design) -> HashMap<String, Vec<ParameterDef>> {
    design
        .tops()
        .map(|top| (top.to_string(), design.parameters(top).unwrap().to_vec()))
        .collect()
}

/// The overridable parameters of every top-level module by module name.
pub fn top_overridable_parameters(
    design: &Design,
) -> HashMap<String, Vec<(String, ParameterKind)>> {
    design
        .tops()
        .map(|top| (top.to_string(), design.overridable_parameters(top).unwrap()))
        .collect()
}

/// The overridable type parameters of every top-level module by module name.
pub fn top_type_parameter_names(design: &Design) -> HashMap<String, Vec<String>> {
    top_overridable_parameters(design)
        .into_iter()
        .map(|(top, parameters)| {
            let names = parameters
                .into_iter()
                .filter(|(_, kind)| *kind == ParameterKind::Type)
                .map(|(name, _)| name)
                .collect();
            (top, names)
        })
        .collect()
}

/// The instance trees of the top-level modules of `cfg` by module name.
pub fn hierarchy(cfg: &SlangConfig) -> Result<HashMap<String, Instance>, SlangError> {
    Ok(run_slang(cfg)?.design.hierarchy())
}
