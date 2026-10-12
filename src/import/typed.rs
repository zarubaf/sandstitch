// SPDX-License-Identifier: Apache-2.0

//! Conversion of Slang port types and locations into library types, and the
//! import diagnostics for ports that cannot be represented.

use regex::Regex;
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

/// A Slang type that cannot be represented as a [`PortType`]: it has
/// unpacked dimensions at the given dotted field path ("" for the port).
struct Unpacked(String);

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

fn fields(path: &str, fields: &[slang_rs::Field]) -> Result<Vec<PortField>, Unpacked> {
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
fn convert_at(path: &str, ty: &slang_rs::Type) -> Result<PortType, Unpacked> {
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
        return Err(Unpacked(path.to_string()));
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
            signed,
            fields: f,
            packed_dimensions,
            ..
        } => PortType::Struct {
            name: qualified_name(name),
            signed: *signed,
            fields: fields(path, f)?,
            packed: dims(packed_dimensions),
        },
        Type::Union {
            name,
            signed,
            fields: f,
            packed_dimensions,
            ..
        } => PortType::Union {
            name: qualified_name(name),
            signed: *signed,
            fields: fields(path, f)?,
            packed: dims(packed_dimensions),
        },
        Type::Enum {
            name,
            variants,
            packed_dimensions,
            ..
        } => {
            // Slang renders every variant as a literal of the base type: its
            // width and signedness are the base's.
            let first = variants
                .first()
                .expect("a SystemVerilog enum has at least one variant");
            let (width, signed) = (first.width, first.signed);
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

pub(crate) fn error(code: crate::Code, message: String, primary: Label) -> Diagnostic {
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

/// Converts the ports of module `module`, given in declaration order. Ports
/// that cannot be represented (unpacked dimensions, or a type that is not
/// integral) are skipped when `skip_unsupported` is set and reported (S0103)
/// otherwise; all such ports are reported together, in declaration order.
pub(crate) fn import_ports(
    module: &str,
    ports: &[Result<slang_rs::Port, slang_rs::UnsupportedPort>],
    skip_unsupported: bool,
) -> Result<Vec<ImportedPort>, ImportError> {
    let mut imported = Vec::new();
    let mut diagnostics = Vec::new();
    for port in ports {
        let port = match port {
            Ok(port) => port,
            Err(unsupported) => {
                let mut d = error(
                    codes::S0103,
                    format!(
                        "port `{}` of `{module}` has unsupported type `{}`",
                        unsupported.name, unsupported.type_text
                    ),
                    Label {
                        span: sv_span(unsupported.loc.as_ref(), &unsupported.name),
                        text: "only integral (bit-vector) types are supported".into(),
                    },
                );
                d.helps.push(
                    "use a packed integral type (logic, bit, int, packed struct, enum)".into(),
                );
                diagnostics.push(d);
                continue;
            }
        };
        let span = sv_span(port.loc.as_ref(), &port.name);
        let ty = match convert_at("", &port.ty) {
            Ok(ty) => ty,
            Err(Unpacked(path)) => {
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
    if diagnostics.is_empty() || skip_unsupported {
        Ok(imported)
    } else {
        Err(ImportError {
            diagnostics: Diagnostics(diagnostics),
        })
    }
}

/// Imports the ports of the top-level instance `module` of an elaborated
/// design: S0104 for interface ports (skipped with `skip_unsupported`), S0103
/// for ports that cannot be represented (see [`import_ports`]). Returns
/// `None` if `module` is not a top-level instance.
pub(crate) fn import_top_ports(
    value: &serde_json::Value,
    module: &str,
    skip_unsupported: bool,
) -> Option<Result<Vec<ImportedPort>, ImportError>> {
    import_instance_ports(value, module, module, skip_unsupported)
}

/// Like [`import_top_ports`], for the instance of `module` at the
/// hierarchical `path` (e.g. `"top.u_child"`).
pub(crate) fn import_instance_ports(
    value: &serde_json::Value,
    path: &str,
    module: &str,
    skip_unsupported: bool,
) -> Option<Result<Vec<ImportedPort>, ImportError>> {
    let interface_ports = slang_rs::extract_instance_interface_ports_from_value(value, path)?;
    if !interface_ports.is_empty() && !skip_unsupported {
        return Some(Err(interface_port_error(module, &interface_ports)));
    }
    let ports = slang_rs::extract_instance_ports_checked_from_value(value, path)?;
    Some(import_ports(module, &ports, skip_unsupported))
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
    let (value, succeeded) = elaborate_definition(module, cfg)?;
    if !succeeded {
        return None;
    }
    slang_rs::extract_interface_ports_from_value(&value).remove(module)
}

/// Elaborates the definition of `module` as the only top, with
/// `--allow-toplevel-iface-ports` so that interface ports do not prevent
/// elaboration. Returns the AST Slang dumped and whether elaboration
/// succeeded; a definition whose default parameters do not elaborate still
/// shows its parameter and port declarations. Returns `None` if Slang dumped
/// no AST.
pub(crate) fn elaborate_definition(
    module: &str,
    cfg: &ParserConfig,
) -> Option<(serde_json::Value, bool)> {
    const FLAG: &str = "--allow-toplevel-iface-ports";
    let mut extra_arguments = cfg.extra_arguments.to_vec();
    // Slang rejects the flag given twice.
    if !extra_arguments.contains(&FLAG) {
        extra_arguments.push(FLAG);
    }
    let cfg = ParserConfig {
        tops: &[module],
        extra_arguments: &extra_arguments,
        ..*cfg
    };
    slang_rs::run_slang_keep_ast(&cfg.to_slang_config()).ok()
}

static SLANG_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(.+?):(\d+):(\d+): error: (.*)$").unwrap());

/// A located error in Slang's output.
pub(crate) struct SlangError {
    /// Absolute path of the file.
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) message: String,
}

/// The text Slang printed for a failed run (`run_slang` reports "slang
/// command failed ..., stderr: <stderr>").
pub(crate) fn slang_output(err: &dyn std::error::Error) -> String {
    let text = err.to_string();
    match text.split_once("stderr: ") {
        Some((_, stderr)) => stderr.to_string(),
        None => text,
    }
}

/// The located errors in Slang's output, in order.
pub(crate) fn slang_errors(text: &str) -> Vec<SlangError> {
    SLANG_ERROR
        .captures_iter(text)
        .map(|c| SlangError {
            file: absolute(&c[1]),
            line: c[2].parse().unwrap_or(0),
            column: c[3].parse().unwrap_or(0),
            message: c[4].to_string(),
        })
        .collect()
}

/// Makes a path from Slang's output absolute (Slang reports paths relative to
/// the directory it ran in, which is this process's working directory).
fn absolute(path: &str) -> String {
    match std::env::current_dir() {
        Ok(cwd) => slang_rs::absolute_path(&cwd, path),
        Err(_) => path.to_string(),
    }
}

/// S0102 for a failed Slang run while importing `module`: the first located
/// error Slang printed is the primary label and the others are secondary
/// labels. Without a located error, Slang's output is the note.
pub(crate) fn slang_failure(module: &str, err: &dyn std::error::Error) -> ImportError {
    let text = slang_output(err);
    let mut labels = slang_errors(&text).into_iter().map(|e| Label {
        span: Span::Sv {
            file: e.file,
            line: e.line,
            column: e.column,
            len: 0,
        },
        text: e.message,
    });
    let primary = labels.next().unwrap_or(Label {
        span: Span::None,
        text: "Slang reported an error".into(),
    });
    let secondary: Vec<Label> = labels.collect();
    let notes = if primary.span == Span::None {
        vec![text.trim().to_string()]
    } else {
        vec![]
    };
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
