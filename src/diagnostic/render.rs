// SPDX-License-Identifier: Apache-2.0

use super::{Diagnostic, Diagnostics, Label, Severity, Span};
use annotate_snippets::{AnnotationKind, Group, Level, Origin, Renderer, Snippet};
use std::ops::Range;
use std::path::Path;

/// Location of a span: file, 1-based line/column (column 0 is treated as 1), and requested length in chars.
fn locate(span: &Span) -> Option<(&str, u32, u32, u32)> {
    match span {
        Span::Rust { file, line, column } => Some((file, *line, (*column).max(1), 0)),
        Span::Sv {
            file,
            line,
            column,
            len,
        } => Some((file, *line, (*column).max(1), *len)),
        Span::None => None,
    }
}

/// Read a source file. Relative paths that do not exist relative to the current directory are
/// retried against each ancestor, since `Location::file()` is workspace-root-relative.
fn read_source(file: &str) -> Option<String> {
    let path = Path::new(file);
    if let Ok(s) = std::fs::read_to_string(path) {
        return Some(s);
    }
    if path.is_absolute() {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .skip(1)
        .find_map(|dir| std::fs::read_to_string(dir.join(path)).ok())
}

/// Byte range of the highlighted text, or None if the position is outside the source.
/// A zero `len` highlights the identifier-like token at the position (at least one char).
/// The column just past the line end gives an empty range.
fn byte_range(src: &str, line: u32, column: u32, len: u32) -> Option<Range<usize>> {
    let mut start_of_line = 0usize;
    for (i, l) in src.split_inclusive('\n').enumerate() {
        if i + 1 == line as usize {
            let text = l.trim_end_matches(['\n', '\r']);
            let idx = column.saturating_sub(1) as usize;
            let s = match text.char_indices().nth(idx) {
                Some((s, _)) => s,
                None if idx == text.chars().count() => text.len(),
                None => return None,
            };
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

    // Sources by file, in order of first appearance.
    let mut files: Vec<(&str, Option<String>)> = Vec::new();
    for (l, _) in &labels {
        if let Some((f, ..)) = locate(&l.span)
            && !files.iter().any(|(n, _)| *n == f)
        {
            files.push((f, read_source(f)));
        }
    }
    let source_of = |f: &str| {
        files
            .iter()
            .find(|(n, _)| *n == f)
            .and_then(|(_, s)| s.as_deref())
    };

    // Labels that can be shown as snippet annotations, and notes for those that cannot.
    let mut located: Vec<Located> = Vec::new();
    let mut unshown: Vec<String> = Vec::new();
    for (l, primary) in &labels {
        let loc = locate(&l.span);
        let range = loc.and_then(|(f, line, column, len)| {
            source_of(f).and_then(|s| byte_range(s, line, column, len))
        });
        match (loc, range) {
            (Some((file, ..)), Some(range)) => located.push(Located {
                file,
                range,
                text: &l.text,
                primary: *primary,
            }),
            (Some((f, line, column, _)), None) => {
                unshown.push(if l.text.is_empty() {
                    format!("{f}:{line}:{column}")
                } else {
                    format!("{f}:{line}:{column}: {}", l.text)
                });
            }
            (None, _) => {
                if !l.text.is_empty() {
                    unshown.push(l.text.clone());
                }
            }
        }
    }

    let mut group = Group::with_title(level.primary_title(&d.message).id(d.code.0));
    // Only the primary label determines the header location.
    if let Some((f, line, column, _)) = locate(&d.primary.span)
        && !located.iter().any(|o| o.primary)
    {
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
    for n in unshown.iter().chain(&d.notes) {
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
