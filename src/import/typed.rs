// SPDX-License-Identifier: Apache-2.0

//! Conversion of Slang port types and locations into library types, and the
//! import diagnostics for ports that cannot be represented.

use regex::Regex;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use crate::diagnostic::codes;
use crate::import::parser_cfg::ParserConfig;
use crate::{
    Diagnostic, Diagnostics, Dim, IO, ImportError, Label, ModDefCore, PortField, PortType,
    QualifiedName, Severity, Span,
};

/// A port converted from Slang, ready for [`ModDefCore::insert_typed_port`].
#[derive(Clone, Debug)]
pub(crate) struct ImportedPort {
    pub(crate) name: String,
    pub(crate) io: fn(usize) -> IO,
    pub(crate) ty: PortType,
    pub(crate) span: Span,
    /// Enum name for unarrayed enum input ports, which need a cast when
    /// emitted (upstream `enum_ports`).
    pub(crate) enum_name: Option<String>,
}

impl ModDefCore {
    /// Inserts imported ports, keeping `enum_ports` in step.
    pub(crate) fn insert_imported_ports(&mut self, ports: &[ImportedPort]) {
        for port in ports {
            self.insert_typed_port(
                port.name.clone(),
                port.io,
                port.ty.clone(),
                port.span.clone(),
            );
            if let Some(enum_name) = &port.enum_name {
                self.enum_ports.insert(port.name.clone(), enum_name.clone());
            }
        }
    }
}

/// Why a Slang type cannot be represented as a [`PortType`].
enum Unsupported {
    /// Unpacked dimensions, at the given dotted field path ("" for the port).
    Unpacked(String),
    /// An enum without variants or with variants of different widths.
    Enum(String),
}

fn join(path: &str, field: &str) -> String {
    if path.is_empty() {
        field.to_string()
    } else {
        format!("{path}.{field}")
    }
}

fn dims(ranges: &[slang_rs::Range]) -> Vec<Dim> {
    ranges
        .iter()
        .map(|r| Dim {
            msb: r.msb.into(),
            lsb: r.lsb.into(),
        })
        .collect()
}

/// Parses a nominal type name from Slang. Slang-generated names of anonymous
/// types contain `$` (`m.s$3`, `cfg_pkg::s$2`) and give `None`, as do names
/// scoped to a module or other hierarchy (`m.local_t`), which cannot be
/// referenced from outside it.
fn qualified_name(name: &str) -> Option<QualifiedName> {
    if name.is_empty() || name.contains('$') {
        return None;
    }
    let (package, name) = match name.split_once("::") {
        Some((package, name)) => (Some(package.to_string()), name),
        None => (None, name),
    };
    if name.contains('.') || name.contains("::") {
        return None;
    }
    Some(QualifiedName {
        package,
        name: name.to_string(),
    })
}

fn fields(path: &str, fields: &[slang_rs::Field]) -> Result<Vec<PortField>, Unsupported> {
    fields
        .iter()
        .map(|f| {
            Ok(PortField {
                name: f.name.clone(),
                ty: convert_at(&join(path, &f.name), &f.ty)?,
            })
        })
        .collect()
}

/// Converts a Slang type. Unpacked dimensions anywhere in the type are
/// rejected.
fn convert_at(path: &str, ty: &slang_rs::Type) -> Result<PortType, Unsupported> {
    use slang_rs::Type;
    let unpacked = match ty {
        Type::Logic {
            unpacked_dimensions,
            ..
        }
        | Type::Struct {
            unpacked_dimensions,
            ..
        }
        | Type::Union {
            unpacked_dimensions,
            ..
        }
        | Type::Enum {
            unpacked_dimensions,
            ..
        } => unpacked_dimensions,
    };
    if !unpacked.is_empty() {
        return Err(Unsupported::Unpacked(path.to_string()));
    }
    Ok(match ty {
        Type::Logic {
            signed,
            packed_dimensions,
            ..
        } => PortType::Logic {
            signed: *signed,
            packed: dims(packed_dimensions),
        },
        Type::Struct {
            name,
            fields: f,
            packed_dimensions,
            ..
        } => PortType::Struct {
            name: qualified_name(name),
            fields: fields(path, f)?,
            packed: dims(packed_dimensions),
        },
        Type::Union {
            name,
            fields: f,
            packed_dimensions,
            ..
        } => PortType::Union {
            name: qualified_name(name),
            fields: fields(path, f)?,
            packed: dims(packed_dimensions),
        },
        Type::Enum {
            name,
            variants,
            packed_dimensions,
            ..
        } => {
            let width = match variants.first() {
                Some(first) if variants.iter().all(|v| v.width == first.width) => first.width,
                _ => return Err(Unsupported::Enum(path.to_string())),
            };
            // Slang's enum text carries no base type, only sized variant
            // values; a base is taken as signed when a value is negative.
            let signed = variants
                .iter()
                .any(|v| v.value.sign() == num_bigint::Sign::Minus);
            PortType::Enum {
                name: qualified_name(name),
                base: Box::new(PortType::Logic {
                    signed,
                    packed: vec![Dim {
                        msb: width as i64 - 1,
                        lsb: 0,
                    }],
                }),
                variants: variants
                    .iter()
                    .map(|v| (v.name.clone(), v.value.clone()))
                    .collect(),
                packed: dims(packed_dimensions),
            }
        }
    })
}

/// A span for a declaration named `name` at `loc`. A missing column (macro
/// expansion) becomes column 0.
pub(crate) fn sv_span(loc: Option<&slang_rs::SourceLoc>, name: &str) -> Span {
    match loc {
        Some(loc) => Span::Sv {
            file: loc.file.clone(),
            line: loc.line,
            column: loc.column.unwrap_or(0),
            len: if loc.column.is_some() {
                name.chars().count() as u32
            } else {
                0
            },
        },
        None => Span::None,
    }
}

fn error(code: crate::Code, message: String, primary: Label) -> Diagnostic {
    Diagnostic {
        code,
        severity: Severity::Error,
        message,
        primary,
        secondary: vec![],
        notes: vec![],
        helps: vec![],
    }
}

/// Converts the ports of module `module`. Ports that cannot be represented
/// are skipped when `skip_unsupported` is set and reported (S0103) otherwise;
/// all such ports are reported together.
pub(crate) fn import_ports(
    module: &str,
    ports: &[slang_rs::Port],
    skip_unsupported: bool,
) -> Result<Vec<ImportedPort>, ImportError> {
    let mut imported = Vec::new();
    let mut diagnostics = Vec::new();
    for port in ports {
        let span = sv_span(port.loc.as_ref(), &port.name);
        let ty = match convert_at("", &port.ty) {
            Ok(ty) => ty,
            Err(_) if skip_unsupported => continue,
            Err(Unsupported::Unpacked(path)) => {
                let mut d = error(
                    codes::S0103,
                    format!("port `{}` of `{module}` has unpacked dimensions", port.name),
                    Label {
                        span,
                        text: "unpacked dimensions are not supported".into(),
                    },
                );
                if !path.is_empty() {
                    d.notes
                        .push(format!("the unpacked dimensions are on field `{path}`"));
                }
                d.helps.push(
                    "declare the port with packed dimensions or split it into separate ports"
                        .into(),
                );
                diagnostics.push(d);
                continue;
            }
            Err(Unsupported::Enum(path)) => {
                let what = if path.is_empty() {
                    "the port's enum type".to_string()
                } else {
                    format!("the enum type of field `{path}`")
                };
                diagnostics.push(error(
                    codes::S0103,
                    format!(
                        "port `{}` of `{module}` has an unsupported enum type",
                        port.name
                    ),
                    Label {
                        span,
                        text: format!("{what} has no variants or variants of different widths"),
                    },
                ));
                continue;
            }
        };
        let io: fn(usize) -> IO = match port.dir {
            slang_rs::PortDir::Input => IO::Input,
            slang_rs::PortDir::Output => IO::Output,
            slang_rs::PortDir::InOut => IO::InOut,
        };
        // Unarrayed enum inputs need a cast when emitted.
        let enum_name = match &port.ty {
            slang_rs::Type::Enum {
                name,
                packed_dimensions,
                ..
            } if packed_dimensions.is_empty() && port.dir == slang_rs::PortDir::Input => {
                Some(name.clone())
            }
            _ => None,
        };
        imported.push(ImportedPort {
            name: port.name.clone(),
            io,
            ty,
            span,
            enum_name,
        });
    }
    if diagnostics.is_empty() {
        Ok(imported)
    } else {
        Err(ImportError {
            diagnostics: Diagnostics(diagnostics),
        })
    }
}

/// S0104 for each interface port of `module`.
pub(crate) fn interface_port_error(module: &str, ports: &[slang_rs::InterfacePort]) -> ImportError {
    let diagnostics = ports
        .iter()
        .map(|port| {
            let ty = match (&port.interface, &port.modport) {
                (i, _) if i.is_empty() => "generic interface".to_string(),
                (i, Some(m)) => format!("`{i}.{m}`"),
                (i, None) => format!("`{i}`"),
            };
            let mut d = error(
                codes::S0104,
                format!("port `{}` of `{module}` is an interface port", port.name),
                Label {
                    span: sv_span(port.loc.as_ref(), &port.name),
                    text: format!("{ty} port is not supported"),
                },
            );
            d.helps.push(
                "replace the interface port with its constituent signals or a packed struct port"
                    .into(),
            );
            d
        })
        .collect();
    ImportError {
        diagnostics: Diagnostics(diagnostics),
    }
}

/// Lists the interface ports of `module` by elaborating the definition itself
/// as the only top with `--allow-toplevel-iface-ports`. A module with
/// interface ports cannot be elaborated as an unconnected top otherwise, nor
/// under a generated wrapper top. Returns `None` if this elaboration fails.
pub(crate) fn interface_ports_of_definition(
    module: &str,
    cfg: &ParserConfig,
) -> Option<Vec<slang_rs::InterfacePort>> {
    let mut extra_arguments = cfg.extra_arguments.to_vec();
    extra_arguments.push("--allow-toplevel-iface-ports");
    let cfg = ParserConfig {
        tops: &[module],
        extra_arguments: &extra_arguments,
        ..*cfg
    };
    let value = slang_rs::run_slang(&cfg.to_slang_config()).ok()?;
    slang_rs::extract_interface_ports_from_value(&value).remove(module)
}

static SLANG_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(.+?):(\d+):(\d+): error: (.*)$").unwrap());

/// Makes a path from Slang's output absolute (Slang reports paths relative to
/// the directory it ran in, which is this process's working directory).
fn absolute(path: &str) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    let Ok(cwd) = std::env::current_dir() else {
        return path.to_string_lossy().into_owned();
    };
    let mut normalized = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized.to_string_lossy().into_owned()
}

/// S0102 for a failed Slang run while importing `module`: every error Slang
/// printed becomes a note; the first located one is the primary label and
/// the others are secondary labels.
pub(crate) fn slang_failure(module: &str, err: &dyn std::error::Error) -> ImportError {
    let text = err.to_string();
    // `run_slang` reports "slang command failed ..., stderr: <stderr>".
    let text = match text.split_once("stderr: ") {
        Some((_, stderr)) => stderr.to_string(),
        None => text,
    };
    let mut labels = SLANG_ERROR.captures_iter(&text).map(|c| Label {
        span: Span::Sv {
            file: absolute(&c[1]),
            line: c[2].parse().unwrap_or(0),
            column: c[3].parse().unwrap_or(0),
            len: 0,
        },
        text: c[4].to_string(),
    });
    let primary = labels.next().unwrap_or(Label {
        span: Span::None,
        text: "Slang reported an error".into(),
    });
    let secondary: Vec<Label> = labels.collect();
    let mut notes: Vec<String> = SLANG_ERROR
        .captures_iter(&text)
        .map(|c| format!("slang: {}", &c[4]))
        .collect();
    if notes.is_empty() {
        notes.push(text.trim().to_string());
    }
    ImportError {
        diagnostics: Diagnostics(vec![Diagnostic {
            code: codes::S0102,
            severity: Severity::Error,
            message: format!("Slang failed to elaborate `{module}`"),
            primary,
            secondary,
            notes,
            helps: vec![],
        }]),
    }
}

/// S0102 for a module that is not a top-level module of the elaborated
/// design (not defined, or instantiated by another module and no `tops`
/// given).
pub(crate) fn module_not_found(module: &str) -> ImportError {
    let mut d = error(
        codes::S0102,
        format!("module definition `{module}` not found in Verilog sources"),
        Label {
            span: Span::None,
            text: "not a top-level module of the elaborated design".into(),
        },
    );
    d.helps.push(format!(
        "check the sources, or select the module with `tops: &[\"{module}\"]`"
    ));
    ImportError {
        diagnostics: Diagnostics(vec![d]),
    }
}
