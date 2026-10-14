// SPDX-License-Identifier: Apache-2.0

//! `ModDef::check`: type checks of typed connections (S0201, S0206) and the
//! connection errors of the resolved design.

use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;

use crate::diagnostic::codes;
use crate::model::connection::port_slice::ConnectionRecord;
use crate::model::port_slice::path_string;
use crate::resolve::design::{DesignResolution, resolve_design};
use crate::{
    Diagnostic, Diagnostics, Label, ModDef, ModDefCore, PortSlice, PortType, Severity, Span, Usage,
};

impl ModDef {
    /// Collects the diagnostics of this module definition and every module
    /// definition it instantiates, directly or indirectly, in the order of
    /// their primary call sites.
    ///
    /// Reports S0206 for each port of an unpacked struct or union type that
    /// would be emitted other than whole through a net of its type (see
    /// `doc/errors/S0206.md`).
    ///
    /// Reports S0201 for each connection between aggregate-typed ends
    /// (structs, unions or enums, as whole typed ports, fields or elements)
    /// whose types are not equivalent. Connections with a logic vector or a
    /// plain bit slice on either end, or made with `connect_bits`, are
    /// checked for width only, when they are made.
    ///
    /// Resolves the connections of this definition, when its internals are
    /// emitted, and of every definition emission descends into, and reports
    /// their errors: S0401 (more than one driver), S0402 (unconnected bits),
    /// S0403 (no driver), S0404 (conflicting intent), S0405 (tieoff or unused
    /// on the wrong direction), S0406 (inout conflict), S0407 (net name
    /// collision) and S0409 (connection cycle).
    ///
    /// Only instantiated definitions whose internals are emitted
    /// (`EmitDefinitionAndDescend`) are descended into.
    ///
    /// Panics if two distinct definitions that emission would emit share a
    /// name.
    pub fn check(&self) -> Diagnostics {
        self.check_design().0
    }

    /// The diagnostics of [`ModDef::check`] and the resolution of the
    /// design, from one pass.
    pub(crate) fn check_design(&self) -> (Diagnostics, DesignResolution) {
        let mut diagnostics = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![self.core.clone()];
        while let Some(core) = stack.pop() {
            if !visited.insert(Arc::as_ptr(&core)) {
                continue;
            }
            // Collect the records first so that no lock on `core` is held
            // while the ends' types are looked up.
            let (records, connections, instances) = {
                let core = core.read();
                let connections = core.mod_def_connections.values().chain(
                    core.mod_inst_connections
                        .values()
                        .flat_map(|ports| ports.values()),
                );
                let (mut records, mut all) = (Vec::new(), Vec::new());
                for c in connections {
                    let c = c.read();
                    records.extend(c.records().iter().cloned());
                    all.extend(c.into_iter().cloned());
                }
                // Definitions whose internals are not emitted are not checked.
                let instances: Vec<Arc<RwLock<ModDefCore>>> = core
                    .instances
                    .values()
                    .rev()
                    .filter(|inst| inst.read().usage == Usage::EmitDefinitionAndDescend)
                    .cloned()
                    .collect();
                (records, all, instances)
            };
            diagnostics.extend(records.iter().filter_map(check_record));
            let is_root = Arc::ptr_eq(&core, &self.core);
            diagnostics.extend(super::unpacked::check_connections(&connections, is_root));
            stack.extend(instances);
        }
        let design = resolve_design(self);
        diagnostics.extend(design.diagnostics.iter().cloned());
        // Report in the order of the call sites.
        diagnostics.sort_by(|a: &Diagnostic, b: &Diagnostic| {
            rust_position(&a.primary.span).cmp(&rust_position(&b.primary.span))
        });
        (Diagnostics(diagnostics), design)
    }
}

fn rust_position(span: &Span) -> (&str, u32, u32) {
    match span {
        Span::Rust { file, line, column } => (file, *line, *column),
        _ => ("", 0, 0),
    }
}

/// The end as the user wrote it: `top.u.flit_i.hdr`.
fn describe(end: &PortSlice) -> String {
    let path = end.select.as_deref().map(path_string).unwrap_or_default();
    format!("{}{path}", end.port.debug_string())
}

fn check_record(record: &ConnectionRecord) -> Option<Diagnostic> {
    if record.bits {
        return None;
    }
    let left = record.this.port_type()?;
    let right = record.other.port_type()?;
    // A logic vector on either end makes the connection width-only, as in
    // upstream: `[8:1]` to `[7:0]`, `wire` to `logic [0:0]`, signed to
    // unsigned, and a struct to an equal-width vector all connect.
    if matches!(left, PortType::Logic { .. }) || matches!(right, PortType::Logic { .. }) {
        return None;
    }
    // An unpacked end is checked by S0206, which also requires the same
    // named type.
    if super::unpacked::is_unpacked(&left) || super::unpacked::is_unpacked(&right) {
        return None;
    }
    let difference = left.equivalent(&right).err()?;
    let note = if difference.path.is_empty() {
        format!("`{}` vs `{}`", difference.left, difference.right)
    } else {
        format!(
            "field `{}`: `{}` vs `{}`",
            difference.path, difference.left, difference.right
        )
    };
    let declared = |end: &PortSlice, ty| Label {
        span: end.port.source_span(),
        text: format!("`{}` is `{ty}`", describe(end)),
    };
    Some(Diagnostic {
        code: codes::S0201,
        severity: Severity::Error,
        message: format!("incompatible types `{left}` and `{right}` in connection"),
        primary: Label {
            span: record.span.clone(),
            text: "connected here".into(),
        },
        secondary: vec![
            declared(&record.this, &left),
            declared(&record.other, &right),
        ],
        notes: vec![note],
        helps: vec![
            "both ends must use equivalent types".into(),
            "to reinterpret bits deliberately, use `connect_bits`".into(),
        ],
        object: None,
    })
}

#[cfg(test)]
mod tests {
    use crate::model::connection::port_slice::ConnectionRecord;
    use crate::{ConvertibleToPortSlice, Dim, IO, ModDef, PortType, Span};

    fn vector(msb: i64) -> PortType {
        PortType::Logic {
            signed: false,
            packed: vec![Dim { msb, lsb: 0 }],
        }
    }

    fn records(m: &ModDef, port: &str) -> Vec<ConnectionRecord> {
        m.get_port(port)
            .get_port_connections()
            .map(|c| c.read().records().to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn records_keep_call_site_and_bits_flag() {
        let m = ModDef::new("m");
        {
            let mut core = m.core.write();
            core.insert_typed_port("a".into(), IO::Input, vector(7), Span::None);
            core.insert_typed_port("b".into(), IO::Output, vector(7), Span::None);
            core.insert_typed_port("c".into(), IO::Output, vector(7), Span::None);
        }
        let plain = m.add_port("plain", IO::Output(8));
        let line = line!() + 1;
        m.get_port("b").connect(&m.get_port("a"));
        m.get_port("c").connect_bits(&m.get_port("a"));
        plain.connect(&m.get_port("a"));

        let b = records(&m, "b");
        assert_eq!(b.len(), 1);
        assert!(!b[0].bits);
        assert_eq!(b[0].this, m.get_port("b").to_port_slice());
        assert_eq!(b[0].other, m.get_port("a").to_port_slice());
        let Span::Rust { file, line: l, .. } = &b[0].span else {
            panic!("{:?}", b[0].span)
        };
        assert!(file.ends_with("src/check/types.rs"), "{file}");
        assert_eq!(*l, line);
        let c = records(&m, "c");
        assert_eq!(c.len(), 1);
        assert!(c[0].bits);
        // Untyped ends are not recorded; records live on the `this` side.
        assert!(records(&m, "plain").is_empty());
        assert!(records(&m, "a").is_empty());
        assert!(!m.check().has_errors());
    }
}
