// SPDX-License-Identifier: Apache-2.0

use super::{Diagnostic, Diagnostics, Label, Severity, Span};
use annotate_snippets::{AnnotationKind, Group, Level, Origin, Renderer, Snippet};
use std::ops::Range;

/// Resolved location: file, 1-based line/column, and requested length in chars.
fn locate(span: &Span) -> Option<(&str, u32, u32, u32)> {
    match span {
        Span::Rust { file, line, column } => Some((file, *line, *column, 0)),
        Span::Sv {
            file,
            line,
            column,
            len,
        } => Some((file, *line, *column, *len)),
        Span::None => None,
    }
}

/// Byte range of the highlighted text, or None if the position is outside the source.
/// A zero `len` highlights the identifier-like token at the position (at least one char).
fn byte_range(src: &str, line: u32, column: u32, len: u32) -> Option<Range<usize>> {
    let mut start_of_line = 0usize;
    for (i, l) in src.split_inclusive('\n').enumerate() {
        if i + 1 == line as usize {
            let text = l.trim_end_matches(['\n', '\r']);
            let (s, _) = text.char_indices().nth(column.saturating_sub(1) as usize)?;
            let rest = &text[s..];
            let n: usize = if len > 0 {
                rest.chars().take(len as usize).map(char::len_utf8).sum()
            } else {
                let tok: usize = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .map(char::len_utf8)
                    .sum();
                if tok > 0 {
                    tok
                } else {
                    rest.chars().next().map_or(0, char::len_utf8)
                }
            };
            return Some(start_of_line + s..start_of_line + s + n);
        }
        start_of_line += l.len();
    }
    None
}

struct Located<'a> {
    file: &'a str,
    range: Range<usize>,
    text: &'a str,
    primary: bool,
}

fn render_one(d: &Diagnostic, renderer: &Renderer) -> String {
    let level = match d.severity {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
    };
    let labels: Vec<(&Label, bool)> = std::iter::once((&d.primary, true))
        .chain(d.secondary.iter().map(|l| (l, false)))
        .collect();

    // Files in order of first appearance; unreadable files fall back to a location line.
    let mut files: Vec<(&str, Option<String>)> = Vec::new();
    for (l, _) in &labels {
        if let Some((f, ..)) = locate(&l.span)
            && !files.iter().any(|(n, _)| *n == f)
        {
            files.push((f, std::fs::read_to_string(f).ok()));
        }
    }
    let source_of = |f: &str| {
        files
            .iter()
            .find(|(n, _)| *n == f)
            .and_then(|(_, s)| s.as_deref())
    };

    let mut located: Vec<Located> = Vec::new();
    let mut origin: Option<(&str, u32, u32)> = None;
    for (l, primary) in &labels {
        if let Some((f, line, column, len)) = locate(&l.span) {
            match source_of(f).and_then(|s| byte_range(s, line, column, len)) {
                Some(range) => located.push(Located {
                    file: f,
                    range,
                    text: &l.text,
                    primary: *primary,
                }),
                None => {
                    origin.get_or_insert((f, line, column));
                }
            }
        }
    }

    let mut group = Group::with_title(level.primary_title(&d.message).id(d.code.0));
    if let Some((f, line, column)) = origin {
        group = group.element(
            Origin::path(f)
                .line(line as usize)
                .char_column(column as usize),
        );
    }
    let mut seen: Vec<&str> = Vec::new();
    for loc in &located {
        if seen.contains(&loc.file) {
            continue;
        }
        seen.push(loc.file);
        let src = source_of(loc.file).expect("located implies readable");
        let annotations = located.iter().filter(|o| o.file == loc.file).map(|o| {
            let kind = if o.primary {
                AnnotationKind::Primary
            } else {
                AnnotationKind::Context
            };
            kind.span(o.range.clone()).label(o.text)
        });
        group = group.element(Snippet::source(src).path(loc.file).annotations(annotations));
    }
    for n in &d.notes {
        group = group.element(Level::NOTE.message(n));
    }
    for h in &d.helps {
        group = group.element(Level::HELP.message(h));
    }
    renderer.render(&[group])
}

pub(super) fn render(ds: &Diagnostics) -> String {
    let renderer = Renderer::plain();
    ds.0.iter()
        .map(|d| render_one(d, &renderer))
        .collect::<Vec<_>>()
        .join("\n\n")
}
