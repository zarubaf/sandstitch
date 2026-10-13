// SPDX-License-Identifier: Apache-2.0

//! Specialization of an imported module. Parameter overrides (integers,
//! strings, types and named constants) are elaborated by Slang under a
//! generated portless top, `__sandstitch_elab_<short_hash>`, that instantiates
//! the module with the overrides; the instance's ports are read with their
//! resolved types. Results are cached by [`SpecializationKey`].

use indexmap::IndexMap;
use num_bigint::{BigInt, Sign};
use parking_lot::RwLock;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::diagnostic::codes;
use crate::import::parser::{VerilogImport, parser_param_to_param};
use crate::import::typed::{self, ImportedPort};
use crate::model::parameter::{ParameterType, ParameterValue};
use crate::{Diagnostics, ImportError, Label, Span};
use slang_rs::ParameterKind;

/// The identity of a specialization: the original module name, the
/// parameter overrides (independent of their order), every field of the
/// import configuration, the Slang version, and the path and content hash of
/// every file Slang read to elaborate it (sources, included headers, library
/// files). Two specializations with equal keys elaborate identically.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpecializationKey {
    digest: [u8; 32],
}

impl SpecializationKey {
    /// The first 12 hex digits of the key's SHA-256 digest, stable across
    /// runs; for provenance.
    pub fn short_hash(&self) -> String {
        short_hash(&self.digest)
    }
}

fn short_hash(digest: &[u8; 32]) -> String {
    digest[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Feeds length-prefixed fields to SHA-256, so that field boundaries are
/// unambiguous.
struct KeyHasher(Sha256);

impl KeyHasher {
    fn new(domain: &str) -> Self {
        let mut hasher = KeyHasher(Sha256::new());
        hasher.str(domain);
        hasher
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.len(bytes.len());
        self.0.update(bytes);
    }

    fn len(&mut self, len: usize) {
        self.0.update((len as u64).to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.bytes(s.as_bytes());
    }

    fn bool(&mut self, b: bool) {
        self.bytes(&[b as u8]);
    }

    fn strs(&mut self, items: &[String]) {
        self.len(items.len());
        for item in items {
            self.str(item);
        }
    }

    fn import(&mut self, import: &VerilogImport) {
        // Destructured so that a new configuration field cannot be missed.
        let VerilogImport {
            sources,
            incdirs,
            defines,
            libfiles,
            libdirs,
            libexts,
            ignore_unknown_modules,
            ignore_protected,
            timescale,
            skip_unsupported,
            extra_arguments,
            // Applied as overrides, which are hashed.
            parameters: _,
            // Derived from the other fields.
            specialization: _,
            // Provenance only; the sources themselves are hashed.
            inline: _,
        } = import;
        self.strs(sources);
        self.strs(incdirs);
        self.len(defines.len());
        for (name, value) in defines {
            self.str(name);
            self.str(value);
        }
        self.strs(libfiles);
        self.strs(libdirs);
        self.strs(libexts);
        self.bool(*ignore_unknown_modules);
        self.bool(*ignore_protected);
        self.bool(timescale.is_some());
        self.str(timescale.as_deref().unwrap_or_default());
        self.bool(*skip_unsupported);
        self.strs(extra_arguments);
        self.str(&slang_version());
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// The Slang library version, which is part of every cache key.
fn slang_version() -> String {
    slang_rs::slang_version()
}

/// The part of a key known before elaboration: everything but the files
/// Slang reads. Overrides are hashed sorted by name, as their order does not
/// change the elaboration.
fn pre_key(
    domain: &str,
    module: &str,
    import: &VerilogImport,
    overrides: &IndexMap<String, ParameterValue>,
) -> [u8; 32] {
    let mut hasher = KeyHasher::new(domain);
    hasher.str(module);
    let mut sorted: Vec<_> = overrides.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    hasher.len(sorted.len());
    for (name, value) in sorted {
        let (kind, text) = match value {
            ParameterValue::Integer(value) => ("integer", value.to_string()),
            ParameterValue::String(value) => ("string", value.clone()),
            ParameterValue::Type(name) => ("type", name.clone()),
            ParameterValue::Expr(name) => ("expr", name.clone()),
        };
        hasher.str(name);
        hasher.str(kind);
        hasher.str(&text);
    }
    hasher.import(import);
    hasher.finish()
}

/// The files an elaboration read, sorted by path, with their content hashes
/// (`None` if unreadable).
type Dependencies = Vec<(String, Option<[u8; 32]>)>;

fn file_digest(path: &str) -> Option<[u8; 32]> {
    std::fs::read(path).ok().map(|c| Sha256::digest(&c).into())
}

/// The files Slang loaded (absolute, headers included), plus the configured
/// sources and library files, without the generated elaboration top.
fn dependencies(
    loaded: &[PathBuf],
    import: &VerilogImport,
    generated: Option<&str>,
) -> Result<Dependencies, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("working directory: {e}"))?;
    let mut paths: Vec<String> = loaded
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .chain(import.sources.iter().cloned())
        .chain(import.libfiles.iter().cloned())
        .map(|path| slang_rs::absolute_path(&cwd, &path))
        .filter(|path| {
            generated.is_none_or(|name| Path::new(path).file_name().is_none_or(|file| file != name))
        })
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths
        .into_iter()
        .map(|path| {
            let digest = file_digest(&path);
            (path, digest)
        })
        .collect())
}

/// A cache whose entries are valid while the files they were elaborated
/// from are unchanged. Keyed by the pre-key; each entry records its
/// dependencies and its final key, hash(pre-key, dependencies).
struct DependencyCache<V> {
    entries: LazyLock<RwLock<CacheEntries<V>>>,
}

type CacheEntries<V> = HashMap<[u8; 32], Vec<CacheEntry<V>>>;

struct CacheEntry<V> {
    dependencies: Dependencies,
    key: [u8; 32],
    value: V,
}

impl<V: Clone> DependencyCache<V> {
    const fn new() -> Self {
        DependencyCache {
            entries: LazyLock::new(|| RwLock::new(HashMap::new())),
        }
    }

    /// The value of an entry for `pre_key` whose dependencies still have the
    /// recorded contents.
    fn get(&self, pre_key: &[u8; 32]) -> Option<V> {
        let entries = self.entries.read();
        entries.get(pre_key)?.iter().find_map(|entry| {
            entry
                .dependencies
                .iter()
                .all(|(path, digest)| file_digest(path) == *digest)
                .then(|| entry.value.clone())
        })
    }

    /// Records `value`. Entries for `pre_key` with the same final key, or
    /// whose dependencies have changed since, are dropped.
    fn insert(&self, pre_key: [u8; 32], dependencies: Dependencies, value: V) {
        let key = final_key(&pre_key, &dependencies);
        let mut entries = self.entries.write();
        let list = entries.entry(pre_key).or_default();
        list.retain(|entry| {
            entry.key != key
                && entry
                    .dependencies
                    .iter()
                    .all(|(path, digest)| file_digest(path) == *digest)
        });
        list.push(CacheEntry {
            dependencies,
            key,
            value,
        });
    }

    #[cfg(test)]
    fn entry_count(&self, pre_key: &[u8; 32]) -> usize {
        self.entries.read().get(pre_key).map_or(0, Vec::len)
    }
}

/// hash(pre-key, dependencies).
fn final_key(pre_key: &[u8; 32], dependencies: &Dependencies) -> [u8; 32] {
    let mut hasher = KeyHasher::new("final");
    hasher.bytes(pre_key);
    hasher.len(dependencies.len());
    for (path, digest) in dependencies {
        hasher.str(path);
        hasher.bool(digest.is_some());
        hasher.bytes(digest.as_ref().map_or(&[][..], |d| &d[..]));
    }
    hasher.finish()
}

/// The result of elaborating a specialization.
#[derive(Clone, Debug)]
pub(crate) struct Specialized {
    pub(crate) key: SpecializationKey,
    pub(crate) ports: Vec<ImportedPort>,
    /// The type of each parameter with an integer override, for emitting the
    /// override as a literal.
    pub(crate) integer_parameter_types: IndexMap<String, ParameterType>,
}

static SPECIALIZATIONS: DependencyCache<Specialized> = DependencyCache::new();

/// What elaborating a module's definition as the top shows. The definition
/// need not elaborate cleanly: the declarations are read from the AST Slang
/// dumps even when its default parameters do not elaborate (a type parameter
/// defaulting to `logic` used as a struct), as long as the sources parse, so
/// overrides are checked before the specialization is elaborated.
#[derive(Clone, Debug)]
struct Definition {
    interface_ports: Vec<slang_rs::InterfacePort>,
    /// Overridable parameters, in declaration order.
    parameters: Vec<(String, ParameterKind)>,
}

/// `None` if the sources do not parse or Slang created no compilation of the
/// definition alone.
static DEFINITIONS: DependencyCache<Option<Definition>> = DependencyCache::new();

/// The instance of the module in the generated elaboration top.
const INSTANCE: &str = "dut";

#[cfg(test)]
thread_local! {
    static ELABORATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The number of specializations this thread has elaborated (cache misses).
#[cfg(test)]
pub(crate) fn elaborations() -> usize {
    ELABORATIONS.with(|count| count.get())
}

static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_$]*$").unwrap());
/// `ident` or `pkg::ident`.
pub(crate) static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_$]*(::[A-Za-z_][A-Za-z0-9_$]*)?$").unwrap());

fn error_at(code: crate::Code, message: String, span: &Span, text: &str) -> crate::Diagnostic {
    typed::error(
        code,
        message,
        Label {
            span: span.clone(),
            text: text.to_string(),
        },
    )
}

fn import_error(diagnostics: Vec<crate::Diagnostic>) -> ImportError {
    ImportError {
        diagnostics: Diagnostics(diagnostics),
    }
}

/// Checks override names (S0101 unless a simple identifier) and values
/// (S0105): types and constants must be `ident` or `pkg::ident`, strings
/// printable ASCII without quotes or backslashes, integers non-negative. All
/// problems are reported together.
pub(crate) fn validate(
    module: &str,
    overrides: &IndexMap<String, ParameterValue>,
    caller: &Span,
) -> Result<(), ImportError> {
    let mut diagnostics = Vec::new();
    for (name, value) in overrides {
        if !IDENTIFIER.is_match(name) {
            let mut d = error_at(
                codes::S0101,
                format!("`{module}` has no overridable parameter `{name}`"),
                caller,
                "overridden here",
            );
            d.notes
                .push("parameter names are simple identifiers".to_string());
            diagnostics.push(d);
            continue;
        }
        let problem = match value {
            ParameterValue::Integer(value) if value.sign() == Sign::Minus => {
                Some(format!("negative integer `{value}` is not supported"))
            }
            ParameterValue::Integer(_) => None,
            ParameterValue::String(value) => (!value
                .bytes()
                .all(|byte| matches!(byte, b' '..=b'~') && !matches!(byte, b'\'' | b'"' | b'\\')))
            .then(|| {
                format!(
                    "string {value:?} must contain only printable ASCII characters and no \
                     single quotes, double quotes or backslashes"
                )
            }),
            ParameterValue::Type(text) | ParameterValue::Expr(text) => (!NAME.is_match(text))
                .then(|| format!("`{text}` is not a name (`ident` or `pkg::ident`)")),
        };
        if let Some(problem) = problem {
            let mut d = error_at(
                codes::S0105,
                format!("unsupported override for parameter `{name}` of `{module}`"),
                caller,
                &problem,
            );
            if matches!(value, ParameterValue::Type(_) | ParameterValue::Expr(_)) {
                d.helps.push(
                    "define a named type or constant in a package and override with \
                     `pkg::name`"
                        .to_string(),
                );
            }
            diagnostics.push(d);
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(import_error(diagnostics))
    }
}

/// Elaborates `module` with `overrides` (validated by [`validate`]), or
/// returns the cached result.
///
/// Errors, before any elaboration top is generated, from elaborating the
/// definition alone: S0104 if the module has interface ports, S0101 for
/// overrides of parameters the module does not declare, S0105 for overrides
/// of the wrong kind (a type for a value parameter or the reverse). Then:
/// S0102 if Slang fails to elaborate the specialization, S0103 for
/// unsupported ports, S0105 for an integer override that does not fit its
/// parameter's type.
pub(crate) fn specialize(
    module: &str,
    import: &VerilogImport,
    overrides: &IndexMap<String, ParameterValue>,
    caller: &Span,
) -> Result<Specialized, ImportError> {
    check_extra_arguments(module, import, caller)?;
    let pre_key = pre_key("specialization", module, import, overrides);
    if let Some(specialized) = SPECIALIZATIONS.get(&pre_key) {
        return Ok(specialized);
    }
    let definition = definition(module, import, caller)?;
    let definition_known = definition.is_some();
    if let Some(definition) = definition {
        if !definition.interface_ports.is_empty() {
            return Err(typed::interface_port_error(
                module,
                &definition.interface_ports,
            ));
        }
        check_against_definition(module, overrides, &definition.parameters, caller)?;
    }
    let (dependencies, ports, integer_parameter_types) = elaborate(
        module,
        import,
        overrides,
        &pre_key,
        definition_known,
        caller,
    )?;
    let specialized = Specialized {
        key: SpecializationKey {
            digest: final_key(&pre_key, &dependencies),
        },
        ports,
        integer_parameter_types,
    };
    SPECIALIZATIONS.insert(pre_key, dependencies, specialized.clone());
    Ok(specialized)
}

/// S0101 for overrides of undeclared parameters, S0105 for overrides of the
/// wrong kind.
fn check_against_definition(
    module: &str,
    overrides: &IndexMap<String, ParameterValue>,
    declared: &[(String, ParameterKind)],
    caller: &Span,
) -> Result<(), ImportError> {
    let mut diagnostics = Vec::new();
    for (name, value) in overrides {
        let Some((_, kind)) = declared.iter().find(|(declared, _)| declared == name) else {
            diagnostics.push(unknown_parameter(module, name, declared, caller));
            continue;
        };
        let is_type = matches!(value, ParameterValue::Type(_));
        let problem = match kind {
            ParameterKind::Value if is_type => Some((
                format!("`{name}` is a value parameter, not a type parameter"),
                "name a constant with `ParameterValue::Expr`, or give an integer or string",
            )),
            ParameterKind::Type if !is_type => Some((
                format!("`{name}` is a type parameter"),
                "name a type with `ParameterValue::Type`",
            )),
            _ => None,
        };
        if let Some((text, help)) = problem {
            let mut d = error_at(
                codes::S0105,
                format!("unsupported override for parameter `{name}` of `{module}`"),
                caller,
                &text,
            );
            d.helps.push(help.to_string());
            diagnostics.push(d);
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(import_error(diagnostics))
    }
}

/// A temporary directory for the generated elaboration top.
fn scratch_dir(module: &str, caller: &Span) -> Result<tempfile::TempDir, ImportError> {
    let dir = tempfile::tempdir()
        .map_err(|e| elaboration_io_failure(module, caller, format!("temporary directory: {e}")))?;
    if dir.path().to_str().is_none() {
        return Err(elaboration_io_failure(
            module,
            caller,
            format!("{} is not valid UTF-8", dir.path().display()),
        ));
    }
    Ok(dir)
}

/// The definition of `module` elaborated alone; `Ok(None)` if the sources do
/// not parse or Slang created no compilation of it.
fn definition(
    module: &str,
    import: &VerilogImport,
    caller: &Span,
) -> Result<Option<Definition>, ImportError> {
    let pre_key = pre_key("definition", module, import, &IndexMap::new());
    if let Some(definition) = DEFINITIONS.get(&pre_key) {
        return Ok(definition);
    }
    let elaborated = import.with_config(&[], &[], &[], |cfg| {
        let output = typed::elaborate_definition(module, cfg)?;
        let definition = output
            .design
            .interface_ports(module)
            .map(|interface_ports| Definition {
                interface_ports: interface_ports.to_vec(),
                parameters: slang_rs::extract_overridable_parameters_from_value(&output.ast)
                    .remove(module)
                    .unwrap_or_default(),
            });
        Some((definition, output.ok, output.deps))
    });
    // No compilation at all: nothing is known, and nothing is cached.
    let Some((definition, succeeded, loaded)) = elaborated else {
        return Ok(None);
    };
    // After a syntax error the AST is Slang's recovery, which can differ
    // from the source (a parameter port list cut short at a typo makes the
    // following parameters local): it is used only if the sources parse.
    let definition = match definition {
        Some(definition) if succeeded || parses(import) => Some(definition),
        _ => None,
    };
    match dependencies(&loaded, import, None) {
        Ok(dependencies) => DEFINITIONS.insert(pre_key, dependencies, definition.clone()),
        Err(e) => return Err(elaboration_io_failure(module, caller, e)),
    }
    Ok(definition)
}

/// Whether Slang parses the sources of `import` without errors.
fn parses(import: &VerilogImport) -> bool {
    import.with_config(&[], &[], &[], |cfg| {
        slang_rs::check_slang(&cfg.to_slang_config()).is_ok()
    })
}

/// S0102 for extra arguments that would clash with those a specialization
/// passes to Slang.
fn check_extra_arguments(
    module: &str,
    import: &VerilogImport,
    caller: &Span,
) -> Result<(), ImportError> {
    // `--Mall` is no longer passed, and the library rejects it: Sandstitch
    // records dependencies itself and never writes dependency files.
    let conflicting = import.extra_arguments.iter().find_map(|argument| {
        slang_rs::BINARY_ONLY_OPTIONS
            .iter()
            .chain(&["--Mall"])
            .find(|option| {
                argument == *option
                    || argument
                        .strip_prefix(**option)
                        .is_some_and(|rest| rest.starts_with('='))
            })
    });
    let Some(option) = conflicting else {
        return Ok(());
    };
    let mut d = error_at(
        codes::S0102,
        format!("cannot elaborate `{module}`"),
        caller,
        "specialized here",
    );
    let reason = if *option == "--Mall" {
        "which would write a dependency file; Sandstitch never writes dependency files"
    } else {
        "which specialization passes to Slang itself or which only the slang binary understands"
    };
    d.notes.push(format!(
        "the import configuration's extra arguments contain `{option}`, {reason}"
    ));
    d.helps.push(format!(
        "remove `{option}` from `ParserConfig::extra_arguments`"
    ));
    Err(import_error(vec![d]))
}

/// Whether the extra arguments set a timescale (`--timescale v` or
/// `--timescale=v`).
fn sets_timescale(extra_arguments: &[String]) -> bool {
    extra_arguments
        .iter()
        .any(|a| a == "--timescale" || a.starts_with("--timescale="))
}

/// S0101 for an override of a parameter `module` does not declare.
fn unknown_parameter(
    module: &str,
    name: &str,
    declared: &[(String, ParameterKind)],
    caller: &Span,
) -> crate::Diagnostic {
    let mut d = error_at(
        codes::S0101,
        format!("`{module}` has no overridable parameter `{name}`"),
        caller,
        "overridden here",
    );
    if declared.is_empty() {
        d.notes
            .push(format!("`{module}` declares no overridable parameters"));
    } else {
        let names: Vec<String> = declared.iter().map(|(n, _)| format!("`{n}`")).collect();
        d.notes
            .push(format!("`{module}` declares {}", names.join(", ")));
    }
    let limit = (name.chars().count() / 3).max(1);
    if let Some((similar, _)) = declared
        .iter()
        .map(|(declared, _)| (declared, edit_distance(name, declared)))
        .filter(|(_, distance)| *distance <= limit)
        .min_by_key(|(_, distance)| *distance)
    {
        d.helps.push(format!("did you mean `{similar}`?"));
    }
    d
}

/// Levenshtein distance, ignoring ASCII case.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current.push(substitute.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// The SystemVerilog text of an override value.
fn override_text(value: &ParameterValue) -> String {
    match value {
        ParameterValue::Integer(value) => {
            // Preserve SystemVerilog's minimum 32-bit width for untyped
            // parameters, including zero. Negative values were rejected.
            let width = value.bits().max(32);
            format!("{width}'d{value}")
        }
        ParameterValue::String(value) => format!("\"{value}\""),
        ParameterValue::Type(name) | ParameterValue::Expr(name) => name.clone(),
    }
}

/// The generated elaboration top `top`, and the override on each line.
fn elaboration_top(
    top: &str,
    module: &str,
    overrides: &IndexMap<String, ParameterValue>,
) -> (String, HashMap<u32, String>) {
    let mut lines = vec![
        format!("// Generated by sandstitch to elaborate `{module}` with parameter overrides."),
        format!("module {top};"),
    ];
    let mut override_lines = HashMap::new();
    if overrides.is_empty() {
        lines.push(format!("  {module} {INSTANCE} ();"));
    } else {
        lines.push(format!("  {module} #("));
        let last = overrides.len() - 1;
        for (i, (name, value)) in overrides.iter().enumerate() {
            let comma = if i == last { "" } else { "," };
            lines.push(format!("    .{name}({}){comma}", override_text(value)));
            override_lines.insert(lines.len() as u32, name.clone());
        }
        lines.push(format!("  ) {INSTANCE} ();"));
    }
    lines.push("endmodule".to_string());
    (lines.join("\n") + "\n", override_lines)
}

/// S0102 at the caller with a note for a failure that is not Slang's.
fn elaboration_io_failure(module: &str, caller: &Span, note: String) -> ImportError {
    let mut d = error_at(
        codes::S0102,
        format!("cannot elaborate `{module}`"),
        caller,
        "specialized here",
    );
    d.notes.push(note);
    import_error(vec![d])
}

/// Whether `value` fits a parameter of type `ty` (non-negative values only).
fn fits(value: &BigInt, ty: &ParameterType) -> bool {
    match ty {
        ParameterType::Signed(width) => value.bits() < *width as u64,
        ParameterType::Unsigned(width) => value.bits() <= *width as u64,
    }
}

/// The files read, the ports and the integer parameter types of an
/// elaboration.
type Elaborated = (
    Dependencies,
    Vec<ImportedPort>,
    IndexMap<String, ParameterType>,
);

/// `definition_known`: the definition's declarations were read
/// ([`definition`]); otherwise the sources may not parse.
fn elaborate(
    module: &str,
    import: &VerilogImport,
    overrides: &IndexMap<String, ParameterValue>,
    pre_key: &[u8; 32],
    definition_known: bool,
    caller: &Span,
) -> Result<Elaborated, ImportError> {
    let top = format!("__sandstitch_elab_{}", short_hash(pre_key));
    let (text, override_lines) = elaboration_top(&top, module, overrides);
    let dir = scratch_dir(module, caller)?;
    let file_name = format!("{top}.sv");
    let path = dir.path().join(&file_name);
    std::fs::write(&path, &text)
        .map_err(|e| elaboration_io_failure(module, caller, format!("{}: {e}", path.display())))?;
    let path = path.to_str().expect("checked by scratch_dir");

    // The generated top leaves every port unconnected; its warnings must not
    // fail a `-Werror` configuration. A module with `timescale needs one on
    // the generated top too.
    let mut extra_arguments = vec!["--suppress-warnings", path];
    if import.timescale.is_none() && !sets_timescale(&import.extra_arguments) {
        extra_arguments.extend(["--timescale", "1ns/1ns"]);
    }

    #[cfg(test)]
    ELABORATIONS.with(|count| count.set(count.get() + 1));
    let output = import
        .with_config(&[&top], &[path], &extra_arguments, |cfg| {
            slang_rs::run_slang(&cfg.to_slang_config())
        })
        .map_err(|e| {
            slang_failure(
                module,
                &e,
                &file_name,
                &override_lines,
                overrides,
                definition_known || parses(import),
                caller,
            )
        })?;
    let dependencies = dependencies(&output.deps, import, Some(&file_name))
        .map_err(|e| elaboration_io_failure(module, caller, e))?;

    let instance = format!("{top}.{INSTANCE}");
    let ports =
        typed::import_instance_ports(&output.design, &instance, module, import.skip_unsupported)
            .ok_or_else(|| module_not_found(module, caller))??;

    let definitions = slang_rs::extract_instance_parameter_defs_from_value(&output.ast, &instance)
        .expect("the instance exists");
    let integer_parameter_types = integer_parameter_types(module, overrides, &definitions, caller)?;
    Ok((dependencies, ports, integer_parameter_types))
}

/// The type of each parameter with an integer override, from the elaborated
/// parameter definitions, for emitting the override as a literal. S0105 for
/// an integer that does not fit its parameter's type, or a parameter whose
/// type has no integer literal form.
pub(crate) fn integer_parameter_types(
    module: &str,
    overrides: &IndexMap<String, ParameterValue>,
    definitions: &[slang_rs::ParameterDef],
    caller: &Span,
) -> Result<IndexMap<String, ParameterType>, ImportError> {
    let mut integer_parameter_types = IndexMap::new();
    let mut diagnostics = Vec::new();
    for (name, value) in overrides {
        let ParameterValue::Integer(value) = value else {
            continue;
        };
        let ty = definitions
            .iter()
            .find(|definition| &definition.name == name)
            .ok_or_else(|| format!("the type of `{name}` has no integer literal form"))
            .and_then(|definition| parser_param_to_param(definition).map(|(_, ty)| ty))
            .and_then(|ty| {
                if fits(value, &ty) {
                    Ok(ty)
                } else {
                    let signedness = if ty.signed() { "signed" } else { "unsigned" };
                    Err(format!(
                        "`{value}` does not fit the {signedness} {}-bit type of `{name}`",
                        ty.width()
                    ))
                }
            });
        match ty {
            Ok(ty) => {
                integer_parameter_types.insert(name.clone(), ty);
            }
            Err(e) => diagnostics.push(error_at(
                codes::S0105,
                format!("unsupported integer override for parameter `{name}` of `{module}`"),
                caller,
                &e,
            )),
        }
    }
    if diagnostics.is_empty() {
        Ok(integer_parameter_types)
    } else {
        Err(import_error(diagnostics))
    }
}

/// The overridable parameters `module` declares, in declaration order;
/// `None` if its definition is not known ([`definition`]).
pub(crate) fn overridable_parameters(
    module: &str,
    import: &VerilogImport,
    caller: &Span,
) -> Result<Option<Vec<(String, ParameterKind)>>, ImportError> {
    Ok(definition(module, import, caller)?.map(|definition| definition.parameters))
}

/// S0102 for a specialization whose module definition was not found (with
/// `ignore_unknown_modules`, Slang elaborates the top without it).
fn module_not_found(module: &str, caller: &Span) -> ImportError {
    let mut d = error_at(
        codes::S0102,
        format!("cannot elaborate `{module}`"),
        caller,
        "specialized here",
    );
    d.notes.push(format!(
        "module definition `{module}` not found in the Verilog sources"
    ));
    d.helps
        .push("check the sources the module was imported with".to_string());
    import_error(vec![d])
}

/// S0102 for a failed Slang run on the elaboration top. Errors in the
/// generated top (which is deleted afterwards) become notes naming the
/// override they are on, unless the sources do not parse (`sources_parse`):
/// the module's parameters are then Slang's recovery, and errors on the
/// overrides are consequences. Errors in the sources are secondary labels.
fn slang_failure(
    module: &str,
    err: &slang_rs::SlangError,
    top_file_name: &str,
    override_lines: &HashMap<u32, String>,
    overrides: &IndexMap<String, ParameterValue>,
    sources_parse: bool,
    caller: &Span,
) -> ImportError {
    let mut d = error_at(
        codes::S0102,
        format!("cannot elaborate `{module}`"),
        caller,
        "specialized here",
    );
    let errors = typed::slang_errors(err);
    if errors.is_empty() {
        d.notes.push(err.rendered.trim().to_string());
    }
    let mut bare_name_undeclared = false;
    for error in errors {
        let in_top = Path::new(&error.file)
            .file_name()
            .is_some_and(|name| name == top_file_name);
        if !in_top {
            d.secondary.push(Label {
                span: Span::Sv {
                    file: error.file,
                    line: error.line,
                    column: error.column,
                    len: 0,
                },
                text: error.message,
            });
            continue;
        }
        if !sources_parse {
            continue;
        }
        match override_lines.get(&error.line) {
            Some(name) => {
                let bare = matches!(
                    &overrides[name],
                    ParameterValue::Type(text) | ParameterValue::Expr(text)
                        if !text.contains("::")
                );
                bare_name_undeclared |= bare && error.message.contains("undeclared identifier");
                d.notes
                    .push(format!("in the override of `{name}`: {}", error.message));
            }
            None => d.notes.push(error.message),
        }
    }
    let rendered: Vec<String> = overrides
        .iter()
        .map(|(name, value)| format!(".{name}({})", override_text(value)))
        .collect();
    d.notes.push(format!(
        "elaborated as `{module} #({}) {INSTANCE} ();`",
        rendered.join(", ")
    ));
    if bare_name_undeclared {
        d.helps.push(
            "a bare name in an override refers to the compilation unit; use `pkg::name` or \
             `--single-unit`"
                .to_string(),
        );
    }
    import_error(vec![d])
}

#[cfg(test)]
mod tests {
    use super::{SPECIALIZATIONS, elaborations, pre_key};
    use crate::model::parameter::ParameterValue;
    use crate::{ModDef, ParserConfig};
    use indexmap::IndexMap;
    use std::path::Path;

    fn specialization_pre_key(base: &ModDef, overrides: &[(&str, i64)]) -> [u8; 32] {
        let core = base.core.read();
        let overrides: IndexMap<String, ParameterValue> = overrides
            .iter()
            .map(|(name, value)| (name.to_string(), (*value).into()))
            .collect();
        pre_key(
            "specialization",
            &core.name,
            core.verilog_import.as_ref().unwrap(),
            &overrides,
        )
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn cache_invalidates_on_source_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("orig.sv");
        let source = "module Orig #(parameter W = 8) (output [W-1:0] d);\nendmodule\n";
        write(&path, source);
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            ..Default::default()
        };
        let base = ModDef::from_verilog_with_config("Orig", &cfg);

        let before = elaborations();
        let first = base.parameterize(&[("W", 4)]);
        assert_eq!(elaborations(), before + 1);
        let hit = base.parameterize(&[("W", 4)]);
        assert_eq!(
            elaborations(),
            before + 1,
            "an equal key must hit the cache"
        );
        assert_eq!(first.specialization_key(), hit.specialization_key());

        // One byte: `W-1` becomes `W-2`.
        let mut content = source.as_bytes().to_vec();
        let at = source.find("W-1").unwrap() + 2;
        content[at] = b'2';
        std::fs::write(&path, &content).unwrap();

        let changed = base.parameterize(&[("W", 4)]);
        assert_eq!(elaborations(), before + 2, "a changed source must miss");
        assert_ne!(first.specialization_key(), changed.specialization_key());
        assert_eq!(first.get_port("d").io().width(), 4);
        assert_eq!(changed.get_port("d").io().width(), 3);
    }

    #[test]
    fn cache_invalidates_on_included_header_change() {
        let dir = tempfile::tempdir().unwrap();
        let inc = dir.path().join("inc");
        std::fs::create_dir(&inc).unwrap();
        let header = inc.join("width.svh");
        write(&header, "`define EXTRA 2\n");
        let path = dir.path().join("hdr.sv");
        write(
            &path,
            "`include \"width.svh\"\nmodule Hdr #(parameter int N = 1) \
             (output logic [N+`EXTRA-1:0] d);\nendmodule\n",
        );
        let sources = [path.to_str().unwrap()];
        let incdirs = [inc.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            incdirs: &incdirs,
            ..Default::default()
        };
        let base = ModDef::from_verilog_with_config("Hdr", &cfg);

        let before = elaborations();
        let first = base.parameterize(&[("N", 4)]);
        assert_eq!(first.get_port("d").io().width(), 6);
        let hit = base.parameterize(&[("N", 4)]);
        assert_eq!(elaborations(), before + 1, "an unchanged header must hit");
        assert_eq!(first.specialization_key(), hit.specialization_key());

        write(&header, "`define EXTRA 5\n");
        let changed = base.parameterize(&[("N", 4)]);
        assert_eq!(elaborations(), before + 2, "a changed header must miss");
        assert_eq!(changed.get_port("d").io().width(), 9);
        assert_ne!(first.specialization_key(), changed.specialization_key());

        // The stale entry was dropped when the new one was recorded.
        let pre_key = specialization_pre_key(&base, &[("N", 4)]);
        assert_eq!(SPECIALIZATIONS.entry_count(&pre_key), 1);

        // Restoring the header re-elaborates and restores the first identity.
        write(&header, "`define EXTRA 2\n");
        let restored = base.parameterize(&[("N", 4)]);
        assert_eq!(elaborations(), before + 3);
        assert_eq!(restored.specialization_key(), first.specialization_key());
        assert_eq!(SPECIALIZATIONS.entry_count(&pre_key), 1);
    }

    #[test]
    fn direct_import_shares_the_cache() {
        // `T` defaults to `logic`, which has no member `v`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("direct.sv");
        write(
            &path,
            "package dp; typedef struct packed { logic v; } t; endpackage\n\
             module Direct #(parameter type T = logic) (input T x, output logic y);\n  \
             assign y = x.v;\nendmodule\n",
        );
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            ..Default::default()
        };
        let overrides = [("T", ParameterValue::Type("dp::t".to_string()))];

        let before = elaborations();
        let first = ModDef::try_from_verilog_parameterized("Direct", &cfg, &overrides).unwrap();
        assert_eq!(elaborations(), before + 1);
        let hit = ModDef::try_from_verilog_parameterized("Direct", &cfg, &overrides).unwrap();
        assert_eq!(elaborations(), before + 1, "equal overrides must hit");
        assert_eq!(first.specialization_key(), hit.specialization_key());

        // Parameterizing the result again with the same overrides is the
        // same specialization.
        let again = first.try_parameterize(&overrides).unwrap();
        assert_eq!(elaborations(), before + 1);
        assert_eq!(again.specialization_key(), first.specialization_key());
    }

    #[test]
    fn cache_invalidates_on_library_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir(&lib).unwrap();
        let leaf = lib.join("leaf.sv");
        write(&leaf, "module leaf (input logic a);\nendmodule\n");
        let path = dir.path().join("uses_leaf.sv");
        write(
            &path,
            "module UsesLeaf #(parameter int N = 1) (input logic [N-1:0] x);\n  \
             leaf u_leaf (.a(x[0]));\nendmodule\n",
        );
        let sources = [path.to_str().unwrap()];
        let libdirs = [lib.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            libdirs: &libdirs,
            libexts: &[".sv"],
            ignore_unknown_modules: false,
            ..Default::default()
        };
        let base = ModDef::from_verilog_with_config("UsesLeaf", &cfg);

        let before = elaborations();
        let first = base.parameterize(&[("N", 2)]);
        base.parameterize(&[("N", 2)]);
        assert_eq!(elaborations(), before + 1);

        write(
            &leaf,
            "module leaf (input logic a);\n  logic unused;\nendmodule\n",
        );
        let changed = base.parameterize(&[("N", 2)]);
        assert_eq!(
            elaborations(),
            before + 2,
            "a changed library file must miss"
        );
        assert_ne!(first.specialization_key(), changed.specialization_key());
    }
}
