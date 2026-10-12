// SPDX-License-Identifier: Apache-2.0

//! rustc-style diagnostics: one model, rendered to terminals and JSON.

pub mod codes;
mod render;

use serde::Serialize;
use std::fmt;

/// A diagnostic code such as `S0201`. Constants live in [`codes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Code(pub &'static str);

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// A source location. Lines and columns are 1-based; columns count characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Span {
    Rust {
        file: String,
        line: u32,
        column: u32,
    },
    Sv {
        file: String,
        line: u32,
        column: u32,
        len: u32,
    },
    None,
}

impl Span {
    /// The location of the caller (requires `#[track_caller]` up the chain).
    #[track_caller]
    pub fn caller() -> Span {
        let loc = std::panic::Location::caller();
        Span::Rust {
            file: loc.file().to_string(),
            line: loc.line(),
            column: loc.column(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Label {
    pub span: Span,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub message: String,
    pub primary: Label,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostic {
    /// Panics with this diagnostic rendered; for errors reported at a Rust
    /// call site.
    pub(crate) fn panic(self) -> ! {
        panic!("{}", Diagnostics(vec![self]).render())
    }
}

impl Diagnostics {
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(|d| d.severity == Severity::Error)
    }

    /// Render all diagnostics in rustc style (plain text, no colors).
    pub fn render(&self) -> String {
        render::render(self)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("diagnostics are serializable")
    }
}

/// Error returned by fallible import/specialization APIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError {
    pub diagnostics: Diagnostics,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.diagnostics.render())
    }
}

impl std::error::Error for ImportError {}
