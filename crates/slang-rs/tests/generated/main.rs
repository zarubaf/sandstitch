// SPDX-License-Identifier: Apache-2.0

//! Generated designs: random packages and module hierarchies are printed as
//! SystemVerilog, elaborated, and every instance's ports and parameters compared
//! with what the model says they are.

mod model;
mod oracle;
mod print;
mod strategy;

use std::collections::HashMap;

use proptest::prelude::*;
use slang_rs::{
    Field, ParameterDef, Port, SlangConfig, Type, UnsupportedPort, run_slang, str2tmpfile,
};

type Ports = Vec<Result<Port, UnsupportedPort>>;

proptest! {
    #[test]
    fn generated_design_matches_model(design in strategy::design()) {
        let sv = print::sv(&design);
        let file = str2tmpfile(&sv).unwrap();
        let path = file.path().to_str().unwrap();
        let cfg = SlangConfig {
            sources: &[path],
            tops: &["top"],
            // No suppressions: "no diagnostics" means none at all.
            ignore_unknown_modules: false,
            ignore_protected: false,
            ..Default::default()
        };
        let output = run_slang(&cfg);
        prop_assert!(
            output.is_ok(),
            "generator defect, the design does not elaborate:\n{}\n{sv}",
            output.unwrap_err()
        );
        let output = output.unwrap();
        prop_assert!(
            output.diagnostics.is_empty(),
            "generator defect, diagnostics:\n{:#?}\n{sv}",
            output.diagnostics
        );
        let mut anonymous = Anonymous::default();
        let expected = oracle::packages(&design);
        let actual = output.design.packages();
        let mut expected_names: Vec<_> = expected.keys().collect();
        let mut actual_names: Vec<_> = actual.keys().collect();
        expected_names.sort();
        actual_names.sort();
        prop_assert_eq!(expected_names, actual_names, "packages differ
{}", sv);
        for (name, expected) in &expected {
            let Some(actual) = actual.get(name) else {
                return Err(TestCaseError::fail(format!("no package {name}\n{sv}")));
            };
            prop_assert_eq!(
                actual.parameters.len(),
                expected.values.len() + expected.aliases.len(),
                "package {} has other members than the model's\n{}",
                name,
                sv
            );
            for (param, value) in &expected.values {
                let found = actual.parameters.get(param);
                prop_assert!(
                    found.is_some_and(|p| &p.value == value && p.ty.is_none()),
                    "{name}::{param} should be {value}, found {found:?}\n{sv}"
                );
            }
            for (alias, ty) in &expected.aliases {
                let found = actual.parameters.get(alias);
                prop_assert!(
                    found.is_some_and(|p| {
                        !p.value.is_empty()
                            && p.ty.as_ref().is_some_and(|actual| {
                                anonymous.same_type(&format!("{name}::{alias}"), ty, actual)
                            })
                    }),
                    "{name}::{alias} should be the type {ty:#?}, found {found:#?}\n{sv}"
                );
            }
        }
        for path in oracle::paths(&design) {
            let expected = oracle::ports(&design, &path);
            let actual = output.design.ports(&path);
            prop_assert!(actual.is_some(), "no instance {path} in the design\n{sv}");
            let actual: Ports = actual
                .unwrap()
                .iter()
                .map(|port| port.clone().map(|port| Port { loc: None, ..port }))
                .collect();
            prop_assert!(
                anonymous.same_ports(&path, &expected, &actual),
                "ports of {path} differ\nexpected: {expected:#?}\nactual: {actual:#?}\n{sv}"
            );
            let expected = oracle::parameters(&design, &path);
            let actual = output.design.parameters(&path);
            prop_assert!(
                actual.is_some_and(|actual| anonymous.same_parameters(&path, &expected, actual)),
                "parameters of {path} differ\nexpected: {expected:#?}\nactual: {actual:#?}\n{sv}"
            );
            let expected = oracle::overridable_parameters(&design, &path);
            let actual = output.design.overridable_parameters(&path);
            prop_assert!(
                actual.as_ref() == Some(&expected),
                "overridable parameters of {path} differ\nexpected: {expected:?}\nactual: {actual:?}\n{sv}"
            );
        }
    }
}

/// Compares types up to the ids of anonymous types: an expected name
/// `scope.s$#7` matches an actual `scope.s$12`, provided that within one
/// instance each model id maps to one Slang id and different model ids to
/// different Slang ids.
#[derive(Default)]
struct Anonymous {
    /// Model name to Slang name, per instance path.
    forward: HashMap<(String, String), String>,
    backward: HashMap<(String, String), String>,
}

impl Anonymous {
    fn same_ports(&mut self, path: &str, expected: &Ports, actual: &Ports) -> bool {
        expected.len() == actual.len()
            && expected.iter().zip(actual).all(|pair| match pair {
                (Ok(e), Ok(a)) => {
                    e.dir == a.dir
                        && e.name == a.name
                        && e.loc == a.loc
                        && self.same_type(path, &e.ty, &a.ty)
                }
                (Err(e), Err(a)) => e == a,
                _ => false,
            })
    }

    fn same_parameters(
        &mut self,
        path: &str,
        expected: &[ParameterDef],
        actual: &[ParameterDef],
    ) -> bool {
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(e, a)| e.name == a.name && self.same_type(path, &e.ty, &a.ty))
    }

    fn same_type(&mut self, path: &str, expected: &Type, actual: &Type) -> bool {
        match (expected, actual) {
            (Type::Logic { .. }, Type::Logic { .. }) => expected == actual,
            (
                Type::Struct {
                    name: en,
                    signed: es,
                    packed: ep,
                    fields: ef,
                    packed_dimensions: epd,
                    unpacked_dimensions: eud,
                },
                Type::Struct {
                    name: an,
                    signed: as_,
                    packed: ap,
                    fields: af,
                    packed_dimensions: apd,
                    unpacked_dimensions: aud,
                },
            )
            | (
                Type::Union {
                    name: en,
                    signed: es,
                    packed: ep,
                    fields: ef,
                    packed_dimensions: epd,
                    unpacked_dimensions: eud,
                },
                Type::Union {
                    name: an,
                    signed: as_,
                    packed: ap,
                    fields: af,
                    packed_dimensions: apd,
                    unpacked_dimensions: aud,
                },
            ) => {
                es == as_
                    && ep == ap
                    && epd == apd
                    && eud == aud
                    && self.same_name(path, en, an)
                    && self.same_fields(path, ef, af)
            }
            (
                Type::Enum {
                    name: en,
                    variants: ev,
                    packed_dimensions: epd,
                    unpacked_dimensions: eud,
                },
                Type::Enum {
                    name: an,
                    variants: av,
                    packed_dimensions: apd,
                    unpacked_dimensions: aud,
                },
            ) => ev == av && epd == apd && eud == aud && self.same_name(path, en, an),
            _ => false,
        }
    }

    fn same_fields(&mut self, path: &str, expected: &[Field], actual: &[Field]) -> bool {
        expected.len() == actual.len()
            && expected
                .iter()
                .zip(actual)
                .all(|(e, a)| e.name == a.name && self.same_type(path, &e.ty, &a.ty))
    }

    fn same_name(&mut self, path: &str, expected: &str, actual: &str) -> bool {
        let Some((prefix, _model_id)) = expected.split_once("$#") else {
            return expected == actual;
        };
        let Some(slang_id) = actual
            .strip_prefix(prefix)
            .and_then(|a| a.strip_prefix('$'))
        else {
            return false;
        };
        if slang_id.is_empty() || !slang_id.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        let forward = self
            .forward
            .entry((path.to_string(), expected.to_string()))
            .or_insert_with(|| actual.to_string());
        let backward = self
            .backward
            .entry((path.to_string(), actual.to_string()))
            .or_insert_with(|| expected.to_string());
        forward == actual && backward == expected
    }
}
