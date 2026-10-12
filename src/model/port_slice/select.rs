// SPDX-License-Identifier: Apache-2.0

//! Typed selections: struct fields and packed-array elements of typed ports.
//!
//! Bit ranges follow the SystemVerilog packed layout: the first declared
//! struct field is the most significant, and element `i` of a dimension
//! `[msb:lsb]` lies `|i - lsb|` elements above the least significant end.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::diagnostic::codes;
use crate::{Code, Diagnostic, Diagnostics, Dim, Label, Port, PortSlice, PortType, Severity, Span};

/// One step of a typed selection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Select {
    /// A struct member, by name.
    Field(String),
    /// An element of the outermost packed dimension, by its SystemVerilog
    /// index.
    Index(usize),
}

/// Why a selection step is not possible.
struct SelectError {
    code: Code,
    message: String,
    notes: Vec<String>,
    helps: Vec<String>,
}

fn packed(ty: &PortType) -> &[Dim] {
    match ty {
        PortType::Logic { packed, .. }
        | PortType::Struct { packed, .. }
        | PortType::Union { packed, .. }
        | PortType::Enum { packed, .. } => packed,
    }
}

/// `ty` with its packed dimensions replaced by `dims`.
fn with_packed(ty: &PortType, dims: Vec<Dim>) -> PortType {
    let mut ty = ty.clone();
    match &mut ty {
        PortType::Logic { packed, .. }
        | PortType::Struct { packed, .. }
        | PortType::Union { packed, .. }
        | PortType::Enum { packed, .. } => *packed = dims,
    }
    ty
}

/// Applies one step to `ty`. Returns the selected type and its bit offset
/// above the least significant bit of `ty`.
fn step(ty: &PortType, select: &Select) -> Result<(PortType, usize), SelectError> {
    match select {
        Select::Index(i) => {
            let dims = packed(ty);
            let Some(outer) = dims.first() else {
                return Err(SelectError {
                    code: codes::S0203,
                    message: format!("cannot index `{ty}`: it has no packed dimension"),
                    notes: vec![],
                    helps: vec!["use `bit` or `slice` to select bits of a vector".into()],
                });
            };
            let (low, high) = (outer.msb.min(outer.lsb), outer.msb.max(outer.lsb));
            let index = i64::try_from(*i).unwrap_or(i64::MAX);
            if index < low || index > high {
                return Err(SelectError {
                    code: codes::S0203,
                    message: format!("index {i} is out of range for `{ty}`"),
                    notes: vec![format!("the outermost dimension is `{outer}`")],
                    helps: vec![],
                });
            }
            let element = with_packed(ty, dims[1..].to_vec());
            let position = (index - outer.lsb).unsigned_abs() as usize;
            let offset = position * element.width();
            Ok((element, offset))
        }
        Select::Field(name) => match ty {
            PortType::Union { .. } => Err(SelectError {
                code: codes::S0204,
                message: format!("cannot select field `{name}` through union `{ty}`"),
                notes: vec!["union members overlap, so a field has no bit range of its own".into()],
                helps: vec![
                    "connect the whole union, or use `connect_bits` to reinterpret bits".into(),
                ],
            }),
            PortType::Struct { packed, .. } if !packed.is_empty() => Err(SelectError {
                code: codes::S0202,
                message: format!("cannot select field `{name}` of `{ty}`: it is a packed array"),
                notes: vec![],
                helps: vec!["select an element with `index` first".into()],
            }),
            PortType::Struct { fields, .. } => {
                let Some(position) = fields.iter().position(|f| f.name == *name) else {
                    let names: Vec<String> =
                        fields.iter().map(|f| format!("`{}`", f.name)).collect();
                    return Err(SelectError {
                        code: codes::S0202,
                        message: format!("no field `{name}` in `{ty}`"),
                        notes: vec![format!("available fields: {}", names.join(", "))],
                        helps: vec![],
                    });
                };
                // Later fields are less significant.
                let offset = fields[position + 1..].iter().map(|f| f.ty.width()).sum();
                Ok((fields[position].ty.clone(), offset))
            }
            _ => Err(SelectError {
                code: codes::S0202,
                message: format!("no field `{name}` in `{ty}`: it is not a struct"),
                notes: vec![],
                helps: vec![],
            }),
        },
    }
}

/// Walks `path` from `root`; returns the selected type and its bit offset
/// above the least significant bit of `root`, or the index of the failing
/// step and why it failed.
fn walk(root: &PortType, path: &[Select]) -> Result<(PortType, usize), (usize, SelectError)> {
    let mut ty = root.clone();
    let mut offset = 0;
    for (i, select) in path.iter().enumerate() {
        let (next, delta) = step(&ty, select).map_err(|e| (i, e))?;
        ty = next;
        offset += delta;
    }
    Ok((ty, offset))
}

/// Renders a selection path after a port name: `.hdr[2].dst_id`.
pub(crate) fn path_string(path: &[Select]) -> String {
    let mut out = String::new();
    for select in path {
        match select {
            Select::Field(name) => write!(out, ".{name}"),
            Select::Index(i) => write!(out, "[{i}]"),
        }
        .expect("writing to a String");
    }
    out
}

impl PortSlice {
    /// Returns the type of this slice: the port's type for a whole port, the
    /// member or element type for a [`field`](Self::field) or
    /// [`index`](Self::index) selection, and `None` for plain bit slices
    /// (`slice`, `bit`, `subdivide`) and for ports without a type.
    pub fn port_type(&self) -> Option<PortType> {
        let path = self.select.as_ref()?;
        let root = self.port.port_type()?;
        Some(
            walk(&root, path)
                .unwrap_or_else(|_| unreachable!("selections are checked when created"))
                .0,
        )
    }

    /// Selects a struct field by a dotted path (`"hdr.dst_id"`). The result is
    /// a typed slice covering the field's bits.
    ///
    /// Panics with a rendered diagnostic if the field does not exist (S0202),
    /// or if the path goes through a union (S0204).
    #[track_caller]
    pub fn field(&self, path: &str) -> PortSlice {
        let steps = path
            .split('.')
            .map(|name| Select::Field(name.to_string()))
            .collect::<Vec<_>>();
        self.select_steps(steps, std::panic::Location::caller())
    }

    /// Selects element `i` of the outermost packed dimension, using its
    /// SystemVerilog index (for `flit_t [3:0]`, `index(0)` is the least
    /// significant element).
    ///
    /// Panics with a rendered diagnostic if `i` is outside the declared range
    /// or the type has no packed dimension (S0203).
    #[track_caller]
    pub fn index(&self, i: usize) -> PortSlice {
        self.select_steps(vec![Select::Index(i)], std::panic::Location::caller())
    }

    fn select_steps(
        &self,
        steps: Vec<Select>,
        location: &'static std::panic::Location<'static>,
    ) -> PortSlice {
        let span = || Span::Rust {
            file: location.file().to_string(),
            line: location.line(),
            column: location.column(),
        };
        let selected = || {
            let what = match &steps[0] {
                Select::Field(_) => "field",
                Select::Index(_) => "index",
            };
            format!("{what} selected here")
        };
        let (Some(base), Some(root)) = (self.select.as_ref(), self.port.port_type()) else {
            let (code, kind) = match &steps[0] {
                Select::Field(_) => (codes::S0202, "fields"),
                Select::Index(_) => (codes::S0203, "elements"),
            };
            let what = if self.select.is_none() {
                format!("`{}` is a plain bit slice", self.debug_string())
            } else {
                format!("`{}` has no type", self.port.debug_string())
            };
            panic_with(Diagnostic {
                code,
                severity: Severity::Error,
                message: format!("cannot select {kind}: {what}"),
                primary: Label {
                    span: span(),
                    text: selected(),
                },
                secondary: vec![],
                notes: vec![],
                helps: vec![
                    "only whole typed ports and their fields and elements have types".into(),
                ],
            });
        };
        let path: Arc<[Select]> = base.iter().cloned().chain(steps.iter().cloned()).collect();
        match walk(&root, &path) {
            Ok((ty, offset)) => PortSlice {
                port: self.port.clone(),
                msb: offset + ty.width() - 1,
                lsb: offset,
                select: Some(path),
            },
            Err((failed, error)) => {
                let at = format!(
                    "{}{}",
                    self.port.debug_string(),
                    path_string(&path[..failed])
                );
                let mut notes = vec![format!("selecting from `{at}`")];
                notes.extend(error.notes);
                panic_with(Diagnostic {
                    code: error.code,
                    severity: Severity::Error,
                    message: error.message,
                    primary: Label {
                        span: span(),
                        text: selected(),
                    },
                    secondary: vec![Label {
                        span: self.port.source_span(),
                        text: format!("`{}` declared here as `{root}`", self.port.name()),
                    }],
                    notes,
                    helps: error.helps,
                })
            }
        }
    }
}

fn panic_with(diagnostic: Diagnostic) -> ! {
    panic!("{}", Diagnostics(vec![diagnostic]).render())
}

impl Port {
    /// Selects a struct field of this typed port by a dotted path
    /// (`"hdr.dst_id"`). See [`PortSlice::field`].
    #[track_caller]
    pub fn field(&self, path: &str) -> PortSlice {
        crate::ConvertibleToPortSlice::to_port_slice(self).field(path)
    }

    /// Selects element `i` of the outermost packed dimension of this typed
    /// port. See [`PortSlice::index`].
    #[track_caller]
    pub fn index(&self, i: usize) -> PortSlice {
        crate::ConvertibleToPortSlice::to_port_slice(self).index(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PortField, QualifiedName};

    fn logic(dims: &[(i64, i64)]) -> PortType {
        PortType::Logic {
            signed: false,
            packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
        }
    }

    fn strukt(fields: &[(&str, PortType)], dims: &[(i64, i64)]) -> PortType {
        PortType::Struct {
            name: Some(QualifiedName {
                package: None,
                name: "s_t".into(),
            }),
            signed: false,
            fields: fields
                .iter()
                .map(|(n, t)| PortField {
                    name: n.to_string(),
                    ty: t.clone(),
                })
                .collect(),
            packed: dims.iter().map(|&(msb, lsb)| Dim { msb, lsb }).collect(),
        }
    }

    fn range(root: &PortType, path: &[Select]) -> (usize, usize) {
        let (ty, offset) = walk(root, path).ok().unwrap();
        (offset + ty.width() - 1, offset)
    }

    fn f(name: &str) -> Select {
        Select::Field(name.into())
    }

    #[test]
    fn ascending_dimension_counts_from_right_bound() {
        // logic [0:3]: index 3 is the least significant bit, index 0 the most.
        let ty = logic(&[(0, 3)]);
        assert_eq!(range(&ty, &[Select::Index(3)]), (0, 0));
        assert_eq!(range(&ty, &[Select::Index(0)]), (3, 3));
    }

    #[test]
    fn offset_dimension_and_nested_struct() {
        // struct { logic [2:0] a; logic [1:0] b; } [5:4]: element width 5.
        let ty = strukt(
            &[("a", logic(&[(2, 0)])), ("b", logic(&[(1, 0)]))],
            &[(5, 4)],
        );
        assert_eq!(range(&ty, &[Select::Index(4)]), (4, 0));
        assert_eq!(range(&ty, &[Select::Index(5), f("a")]), (9, 7));
        assert_eq!(range(&ty, &[Select::Index(5), f("b")]), (6, 5));
        assert!(walk(&ty, &[Select::Index(3)]).is_err());
        assert!(walk(&ty, &[f("a")]).is_err());
    }
}
