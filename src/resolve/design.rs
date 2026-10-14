// SPDX-License-Identifier: Apache-2.0

//! Resolution of every definition that emission plans, in one pass, and the
//! comparison of definitions that share a key.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use super::Resolution;
use super::coexist::{DefView, first_difference, is_emitted};
use super::def::resolve_def;
use super::key::{DefKey, def_key};
use crate::diagnostic::codes;
use crate::{Diagnostic, Label, ModDef, ModDefCore, Severity, Span, Usage};

type CorePtr = *const RwLock<ModDefCore>;

/// The resolutions of a design's definitions, keyed by core identity, the
/// definitions to emit, and the diagnostics of all of them, in resolution
/// order.
pub(crate) struct DesignResolution {
    resolutions: HashMap<CorePtr, Resolution>,
    collapsed: HashSet<CorePtr>,
    emitted: Vec<Arc<RwLock<ModDefCore>>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl DesignResolution {
    /// The resolution of `core`, if emission plans it as a definition.
    pub(crate) fn get(&self, core: &Arc<RwLock<ModDefCore>>) -> Option<&Resolution> {
        self.resolutions.get(&Arc::as_ptr(core))
    }

    /// Whether `core` equals an earlier definition with its key and so is
    /// emitted as that one.
    pub(crate) fn collapsed(&self, core: &Arc<RwLock<ModDefCore>>) -> bool {
        self.collapsed.contains(&Arc::as_ptr(core))
    }

    /// The definitions to emit (`EmitDefinitionAndDescend` and
    /// `EmitStubAndStop`), children before parents, in first-seen order, one
    /// per key.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by the emitter (Task 3)"))]
    pub(crate) fn emitted(&self) -> &[Arc<RwLock<ModDefCore>>] {
        &self.emitted
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.resolutions.len()
    }
}

/// Resolves, children before parents and each once, every definition
/// emission plans: reached from `top` as emission descends, with usage
/// `EmitDefinitionAndDescend`. Registers every reachable definition by key
/// and by module name, and reports S0408 for two different definitions with
/// one key and for an emitted definition whose module name another key has.
pub(crate) fn resolve_design(top: &ModDef) -> DesignResolution {
    let mut walk = Walk {
        design: DesignResolution {
            resolutions: HashMap::new(),
            collapsed: HashSet::new(),
            emitted: Vec::new(),
            diagnostics: Vec::new(),
        },
        seen: HashSet::new(),
        keys: HashMap::new(),
        names: HashMap::new(),
    };
    walk.visit(&top.core, String::new());
    walk.design
}

/// The first definition registered under a key.
struct Keyed {
    core: Arc<RwLock<ModDefCore>>,
    path: String,
    /// Whether its resolution, if it has one, is error-free.
    clean: bool,
    /// Its view, built when a second definition with its key appears.
    view: Option<DefView>,
}

/// The first definition registered under a module name.
struct Named {
    key: DefKey,
    core: Arc<RwLock<ModDefCore>>,
    path: String,
    emitted: bool,
}

struct Walk {
    design: DesignResolution,
    seen: HashSet<CorePtr>,
    keys: HashMap<DefKey, Keyed>,
    names: HashMap<String, Named>,
}

impl Walk {
    /// Visits `core`, first reached at instance path `path` (empty for the
    /// top): its children, then itself.
    fn visit(&mut self, core: &Arc<RwLock<ModDefCore>>, path: String) {
        if !self.seen.insert(Arc::as_ptr(core)) {
            return;
        }
        // Resolving locks cores, so no guard is held past this block.
        let (usage, children) = {
            let guard = core.read();
            let children: Vec<_> = if guard.usage == Usage::EmitDefinitionAndDescend {
                guard
                    .instances
                    .iter()
                    .map(|(name, child)| (name.clone(), child.clone()))
                    .collect()
            } else {
                Vec::new()
            };
            (guard.usage.clone(), children)
        };

        for (name, child) in &children {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            self.visit(child, child_path);
        }

        let key = def_key(&core.read());
        let first = !self.keys.contains_key(&key);
        if first {
            let clean = usage != Usage::EmitDefinitionAndDescend || self.resolve(core);
            let keyed = Keyed {
                core: core.clone(),
                path: path.clone(),
                clean,
                view: None,
            };
            self.keys.insert(key.clone(), keyed);
        } else {
            self.compare(&key, core, &path);
        }
        self.register_name(key, core, path, is_emitted(&usage));
        if first && is_emitted(&usage) {
            self.design.emitted.push(core.clone());
        }
    }

    /// Resolves `core`, keeping its resolution and diagnostics; returns
    /// whether it is error-free.
    fn resolve(&mut self, core: &Arc<RwLock<ModDefCore>>) -> bool {
        let (resolution, diagnostics) = resolve_def(core);
        let clean = diagnostics.is_empty();
        self.design.diagnostics.extend(diagnostics);
        let previous = self
            .design
            .resolutions
            .insert(Arc::as_ptr(core), resolution);
        debug_assert!(previous.is_none(), "definition resolved twice");
        clean
    }

    /// Compares `core` with the first definition registered under `key`:
    /// collapses it into that one if equal, reports S0408 if different. The
    /// duplicate is resolved only when everything but its connections is
    /// equal; its resolution is then dropped.
    fn compare(&mut self, key: &DefKey, core: &Arc<RwLock<ModDefCore>>, path: &str) {
        let first = self.keys.get_mut(key).expect("the key is registered");
        let resolutions = &self.design.resolutions;
        let first_view = first.view.get_or_insert_with(|| {
            let mut view = DefView::new(&first.core);
            if view.descends() && first.clean {
                view.set_connections(&resolutions[&Arc::as_ptr(&first.core)]);
            }
            view
        });
        let mut view = DefView::new(core);
        let mut field = first_difference(first_view, &view);
        if field.is_none() && view.descends() {
            let (resolution, diagnostics) = resolve_def(core);
            if !diagnostics.is_empty() {
                // The user's own errors; the connections are not compared.
                self.design.diagnostics.extend(diagnostics);
                return;
            }
            if !first_view.has_connections() {
                // The first definition has errors, reported already.
                return;
            }
            view.set_connections(&resolution);
            field = first_difference(first_view, &view);
        }
        match field {
            None => {
                self.design.collapsed.insert(Arc::as_ptr(core));
            }
            Some(field) => {
                let diagnostic = s0408(
                    format!("two different definitions `{key}`: they differ in {field}"),
                    (&first.core, &first.path),
                    (core, path),
                );
                self.design.diagnostics.push(diagnostic);
            }
        }
    }

    /// Registers the module name of `core`; another key under the same name
    /// is S0408 when either definition is emitted.
    fn register_name(
        &mut self,
        key: DefKey,
        core: &Arc<RwLock<ModDefCore>>,
        path: String,
        emitted: bool,
    ) {
        let name = core.read().name.clone();
        match self.names.get(&name) {
            None => {
                let named = Named {
                    key,
                    core: core.clone(),
                    path,
                    emitted,
                };
                self.names.insert(name, named);
            }
            Some(named) if named.key != key && (named.emitted || emitted) => {
                let diagnostic = s0408(
                    format!(
                        "definitions `{}` and `{key}` share the module name `{name}`, and at least one is emitted",
                        named.key
                    ),
                    (&named.core, &named.path),
                    (core, &path),
                );
                self.design.diagnostics.push(diagnostic);
            }
            Some(_) => {}
        }
    }
}

/// S0408 for an earlier and a later definition, each with the instance path
/// that first uses it.
fn s0408(
    message: String,
    earlier: (&Arc<RwLock<ModDefCore>>, &str),
    later: (&Arc<RwLock<ModDefCore>>, &str),
) -> Diagnostic {
    let site = |(core, path): (&Arc<RwLock<ModDefCore>>, &str)| {
        let core = core.read();
        let path = if path.is_empty() { &core.name } else { path };
        (core.span.clone(), format!("used at {path}"))
    };
    let (earlier_span, earlier_note) = site(earlier);
    let (later_span, later_note) = site(later);
    let secondary = if earlier_span == Span::None {
        Vec::new()
    } else {
        vec![Label {
            span: earlier_span,
            text: "first defined here".to_string(),
        }]
    };
    Diagnostic {
        code: codes::S0408,
        severity: Severity::Error,
        message,
        primary: Label {
            span: later_span,
            text: "defined here".to_string(),
        },
        secondary,
        notes: vec![earlier_note, later_note],
        helps: vec!["give one of the definitions another name".to_string()],
        object: None,
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_design;
    use crate::diagnostic::codes;
    use crate::{Diagnostic, IO, ModDef, ParserConfig, Usage};

    fn opaque(name: &str, port: &str, io: IO) -> ModDef {
        let leaf = ModDef::new(name);
        leaf.add_port(port, io);
        leaf.set_usage(Usage::EmitNothingAndStop);
        leaf
    }

    /// A top instantiating each of `defs` as `a`, `b`, ... (`d<n>` from the
    /// 27th on), with the instances' inputs tied off and their outputs
    /// marked unused.
    fn top_of(defs: &[&ModDef]) -> ModDef {
        let top = ModDef::new("top");
        for (n, def) in defs.iter().enumerate() {
            let name = match u8::try_from(n) {
                Ok(n) if n < 26 => char::from(b'a' + n).to_string(),
                _ => format!("d{n}"),
            };
            let inst = top.instantiate(def, Some(&name), None);
            let ports: Vec<(String, IO)> = def
                .core
                .read()
                .ports
                .iter()
                .map(|(port, io)| (port.clone(), io.clone()))
                .collect();
            for (port, io) in ports {
                match io {
                    IO::Input(_) => inst.get_port(&port).tieoff(0),
                    _ => inst.get_port(&port).unused(),
                }
            }
        }
        top
    }

    fn s0408s(diagnostics: &[Diagnostic]) -> Vec<(&str, &[String])> {
        diagnostics
            .iter()
            .filter(|d| d.code == codes::S0408)
            .map(|d| (d.message.as_str(), d.notes.as_slice()))
            .collect()
    }

    /// The emitted definitions' module names.
    fn emitted_names(top: &ModDef) -> Vec<String> {
        resolve_design(top)
            .emitted()
            .iter()
            .map(|core| core.read().name.clone())
            .collect()
    }

    /// Imports module `Keyed` (a `W`-bit output `d`) with the Slang `-G`
    /// overrides `parameters`.
    fn import_keyed(dir: &tempfile::TempDir, parameters: &[(&str, &str)]) -> ModDef {
        let path = dir.path().join("keyed.sv");
        std::fs::write(
            &path,
            "module Keyed #(parameter W = 8) (output [W-1:0] d);\nendmodule\n",
        )
        .unwrap();
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            parameters,
            ..Default::default()
        };
        ModDef::from_verilog_with_config("Keyed", &cfg)
    }

    #[test]
    fn resolves_each_planned_definition_once() {
        let leaf = opaque("leaf", "i", IO::Input(1));
        let child = ModDef::new("child");
        child.add_port("i", IO::Input(1));
        let c_leaf = child.instantiate(&leaf, Some("l"), None);
        child.get_port("i").connect(&c_leaf.get_port("i"));

        let top = ModDef::new("top");
        top.add_port("i", IO::Input(1));
        let c0 = top.instantiate(&child, Some("c0"), None);
        let c1 = top.instantiate(&child, Some("c1"), None);
        top.get_port("i").connect(&c0.get_port("i"));
        top.get_port("i").connect(&c1.get_port("i"));
        let l = top.instantiate(&leaf, Some("l"), None);
        top.get_port("i").connect(&l.get_port("i"));

        let design = resolve_design(&top);
        assert!(design.get(&top.core).is_some());
        assert!(design.get(&child.core).is_some());
        assert!(design.get(&leaf.core).is_none());
        assert_eq!(design.len(), 2);
    }

    #[test]
    fn children_resolve_before_parents() {
        let leaf = opaque("leaf", "i", IO::Input(1));
        let child = ModDef::new("child");
        child.instantiate(&leaf, Some("l"), None);
        let top = ModDef::new("top");
        top.add_port("o", IO::Output(1));
        top.instantiate(&child, Some("c"), None);
        let design = resolve_design(&top);
        let found: Vec<_> = design
            .diagnostics
            .iter()
            .map(|d| {
                let o = d.object.as_ref().unwrap();
                (d.code, o.definition.as_str(), o.end.as_str(), o.msb, o.lsb)
            })
            .collect();
        assert_eq!(
            found,
            [
                (codes::S0402, "child", "l.i", 0, 0),
                (codes::S0402, "top", "o", 0, 0)
            ]
        );
    }

    #[test]
    fn different_definitions_with_one_key_report_s0408() {
        let first = ModDef::new("leaf");
        first.add_port("i", IO::Input(1)).unused();
        let second = ModDef::new("leaf");
        second.add_port("i", IO::Input(2)).unused();
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        assert_eq!(design.diagnostics.len(), 1, "{:?}", design.diagnostics);
        let d = &design.diagnostics[0];
        assert_eq!(d.code, codes::S0408);
        assert_eq!(
            d.message,
            "two different definitions `leaf`: they differ in ports"
        );
        assert_eq!(d.notes, ["used at a", "used at b"]);
        assert!(d.object.is_none());
        assert_eq!(d.primary.text, "defined here");
        assert_eq!(d.primary.span, second.core.read().span);
        assert_eq!(d.secondary.len(), 1);
        assert_eq!(d.secondary[0].text, "first defined here");
        assert_eq!(d.secondary[0].span, first.core.read().span);
    }

    #[test]
    fn equal_definitions_with_one_key_emit_once() {
        let build = || {
            let leaf = ModDef::new("leaf");
            leaf.add_port("o", IO::Output(1)).tieoff(1);
            leaf
        };
        let (first, second) = (build(), build());
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        assert!(design.diagnostics.is_empty(), "{:?}", design.diagnostics);
        assert!(design.collapsed(&second.core));
        assert!(!design.collapsed(&first.core));
        let emitted = design.emitted();
        assert_eq!(emitted.len(), 2);
        assert!(std::sync::Arc::ptr_eq(&emitted[0], &first.core));
        assert!(std::sync::Arc::ptr_eq(&emitted[1], &top.core));
    }

    #[test]
    fn same_core_under_two_parents_is_not_compared() {
        let leaf = ModDef::new("leaf");
        leaf.add_port("o", IO::Output(1)).tieoff(0);
        let p = ModDef::new("p");
        p.instantiate(&leaf, Some("l"), None).get_port("o").unused();
        let q = ModDef::new("q");
        q.instantiate(&leaf, Some("l"), None).get_port("o").unused();
        let top = top_of(&[&p, &q]);
        let design = resolve_design(&top);
        assert!(design.diagnostics.is_empty(), "{:?}", design.diagnostics);
        assert_eq!(emitted_names(&top), ["leaf", "p", "q", "top"]);
    }

    #[test]
    fn different_children_under_equal_parents_report_s0408() {
        let parent = |value: u32| {
            let leaf = ModDef::new("leaf");
            leaf.add_port("o", IO::Output(1)).tieoff(value);
            let p = ModDef::new("p");
            let o = p.add_port("o", IO::Output(1));
            o.connect(&p.instantiate(&leaf, Some("l"), None).get_port("o"));
            p
        };
        let (a, b) = (parent(0), parent(1));
        let top = top_of(&[&a, &b]);
        let design = resolve_design(&top);
        assert_eq!(
            s0408s(&design.diagnostics),
            [(
                "two different definitions `leaf`: they differ in connections",
                ["used at a.l".to_string(), "used at b.l".to_string()].as_slice()
            )]
        );
        assert_eq!(design.diagnostics.len(), 1, "{:?}", design.diagnostics);
        assert!(design.collapsed(&b.core));
    }

    #[test]
    fn duplicate_errors_are_reported() {
        let sink = opaque("sink", "i", IO::Input(1));
        let build = |driven: bool| {
            let leaf = ModDef::new("leaf");
            let i = leaf.instantiate(&sink, Some("u"), None).get_port("i");
            if driven {
                i.tieoff(0);
            } else {
                i.specify_net_name("data");
            }
            leaf
        };
        let (first, second) = (build(true), build(false));
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        let found: Vec<_> = design
            .diagnostics
            .iter()
            .map(|d| (d.code, d.object.as_ref().map(|o| o.definition.as_str())))
            .collect();
        assert_eq!(found, [(codes::S0403, Some("leaf"))]);
    }

    #[test]
    fn emitted_and_opaque_with_one_module_name_report_s0408() {
        let dir = tempfile::tempdir().unwrap();
        let imported = import_keyed(&dir, &[("W", "4")]);
        let key = super::def_key(&imported.core.read()).to_string();
        assert!(key.starts_with("Keyed#"), "{key}");
        let generated = ModDef::new("Keyed");
        let top = top_of(&[&generated, &imported]);
        let design = resolve_design(&top);
        let message = format!(
            "definitions `Keyed` and `{key}` share the module name `Keyed`, and at least one is emitted"
        );
        assert_eq!(
            s0408s(&design.diagnostics),
            [(
                message.as_str(),
                ["used at a".to_string(), "used at b".to_string()].as_slice()
            )]
        );
        assert_eq!(design.diagnostics.len(), 1, "{:?}", design.diagnostics);
        // The import is the later definition and has no Rust span.
        assert_eq!(design.diagnostics[0].primary.span, crate::Span::None);
        assert_eq!(design.diagnostics[0].secondary.len(), 1);
    }

    #[test]
    fn opaque_definitions_compare_ports() {
        let first = opaque("leaf", "o", IO::Output(1));
        let narrow = opaque("leaf", "o", IO::Output(1));
        let wide = opaque("leaf", "o", IO::Output(2));
        let top = top_of(&[&first, &narrow]);
        assert!(resolve_design(&top).diagnostics.is_empty());
        let top = top_of(&[&first, &wide]);
        let design = resolve_design(&top);
        assert_eq!(
            s0408s(&design.diagnostics),
            [(
                "two different definitions `leaf`: they differ in ports",
                ["used at a".to_string(), "used at b".to_string()].as_slice()
            )]
        );
        assert_eq!(design.diagnostics.len(), 1, "{:?}", design.diagnostics);
    }

    #[test]
    fn stubbed_specializations_with_one_name_report_s0408() {
        let dir = tempfile::tempdir().unwrap();
        let base = import_keyed(&dir, &[]);
        let w4 = base.try_parameterize(&[("W", 4)]).unwrap();
        let w6 = base.try_parameterize(&[("W", 6)]).unwrap();
        w4.set_usage(Usage::EmitStubAndStop);
        w6.set_usage(Usage::EmitStubAndStop);
        let key4 = super::def_key(&w4.core.read()).to_string();
        let key6 = super::def_key(&w6.core.read()).to_string();
        let top = top_of(&[&w4, &w6]);
        let design = resolve_design(&top);
        let message = format!(
            "definitions `{key4}` and `{key6}` share the module name `Keyed`, and at least one is emitted"
        );
        assert_eq!(
            s0408s(&design.diagnostics),
            [(
                message.as_str(),
                ["used at a".to_string(), "used at b".to_string()].as_slice()
            )]
        );
        assert_eq!(design.diagnostics.len(), 1, "{:?}", design.diagnostics);
    }

    #[test]
    fn specializations_of_one_module_coexist() {
        let dir = tempfile::tempdir().unwrap();
        let base = import_keyed(&dir, &[]);
        let w4 = base.try_parameterize(&[("W", 4)]).unwrap();
        let w6 = base.try_parameterize(&[("W", 6)]).unwrap();
        let top = top_of(&[&w4, &w6]);
        let design = resolve_design(&top);
        assert!(design.diagnostics.is_empty(), "{:?}", design.diagnostics);
        assert_eq!(emitted_names(&top), ["top"]);
    }

    /// A `leaf` joining a producer's struct output to a consumer's input of
    /// a struct type with another layout, with `connect_bits` or `connect`
    /// (S0201).
    fn struct_leaf(dir: &tempfile::TempDir, bits: bool) -> ModDef {
        let path = dir.path().join("bus.sv");
        std::fs::write(
            &path,
            "package bus_pkg;\n\
             typedef struct packed { logic [31:0] addr; logic [7:0] id; } req_t;\n\
             typedef struct packed { logic [7:0] id; logic [31:0] addr; } req2_t;\n\
             endpackage\n\
             module producer import bus_pkg::*; (output req_t req_o); endmodule\n\
             module consumer import bus_pkg::*; (input req2_t req_i); endmodule\n",
        )
        .unwrap();
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            ..Default::default()
        };
        let producer = ModDef::from_verilog_with_config("producer", &cfg);
        let consumer = ModDef::from_verilog_with_config("consumer", &cfg);
        let leaf = ModDef::new("leaf");
        let p = leaf.instantiate(&producer, Some("p"), None);
        let c = leaf.instantiate(&consumer, Some("c"), None);
        if bits {
            p.get_port("req_o").connect_bits(&c.get_port("req_i"));
        } else {
            p.get_port("req_o").connect(&c.get_port("req_i"));
        }
        leaf
    }

    #[test]
    fn connect_bits_and_connect_differ_in_connections() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (struct_leaf(&dir, true), struct_leaf(&dir, false));
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        assert_eq!(
            s0408s(&design.diagnostics),
            [(
                "two different definitions `leaf`: they differ in connections",
                ["used at a".to_string(), "used at b".to_string()].as_slice()
            )]
        );
        assert!(!design.collapsed(&second.core));
        let codes: Vec<_> = top.check().0.iter().map(|d| d.code).collect();
        assert_eq!(codes, [codes::S0408, codes::S0201]);
    }

    #[test]
    fn connection_order_and_direction_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("same.sv");
        std::fs::write(
            &path,
            "package same_pkg;\n\
             typedef struct packed { logic [31:0] addr; logic [7:0] id; } req_t;\n\
             endpackage\n\
             module producer import same_pkg::*; (output req_t req_o); endmodule\n\
             module consumer import same_pkg::*; (input req_t req_i); endmodule\n",
        )
        .unwrap();
        let sources = [path.to_str().unwrap()];
        let cfg = ParserConfig {
            sources: &sources,
            ..Default::default()
        };
        let producer = ModDef::from_verilog_with_config("producer", &cfg);
        let consumer = ModDef::from_verilog_with_config("consumer", &cfg);
        let build = |reversed: bool| {
            let leaf = ModDef::new("leaf");
            let p0 = leaf.instantiate(&producer, Some("p0"), None);
            let c0 = leaf.instantiate(&consumer, Some("c0"), None);
            let p1 = leaf.instantiate(&producer, Some("p1"), None);
            let c1 = leaf.instantiate(&consumer, Some("c1"), None);
            if reversed {
                c1.get_port("req_i").connect(&p1.get_port("req_o"));
                c0.get_port("req_i").connect(&p0.get_port("req_o"));
            } else {
                p0.get_port("req_o").connect(&c0.get_port("req_i"));
                p1.get_port("req_o").connect(&c1.get_port("req_i"));
            }
            leaf
        };
        let (first, second) = (build(false), build(true));
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        assert!(design.diagnostics.is_empty(), "{:?}", design.diagnostics);
        assert!(design.collapsed(&second.core));
    }

    #[test]
    fn equal_duplicates_report_type_errors_once() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (struct_leaf(&dir, false), struct_leaf(&dir, false));
        let top = top_of(&[&first, &second]);
        assert!(resolve_design(&top).collapsed(&second.core));
        let codes: Vec<_> = top.check().0.iter().map(|d| d.code).collect();
        assert_eq!(codes, [codes::S0201]);
    }

    #[test]
    fn first_errors_are_reported_without_s0408() {
        let sink = opaque("sink", "i", IO::Input(1));
        let build = |driven: bool| {
            let leaf = ModDef::new("leaf");
            let i = leaf.instantiate(&sink, Some("u"), None).get_port("i");
            if driven {
                i.tieoff(0);
            } else {
                i.specify_net_name("data");
            }
            leaf
        };
        let (first, second) = (build(false), build(true));
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        let found: Vec<_> = design
            .diagnostics
            .iter()
            .map(|d| (d.code, d.object.as_ref().map(|o| o.definition.as_str())))
            .collect();
        assert_eq!(found, [(codes::S0403, Some("leaf"))]);
        assert!(!design.collapsed(&second.core));
    }

    #[test]
    fn stubs_compare_only_their_interface() {
        let sink = opaque("sink", "i", IO::Input(1));
        let first = ModDef::new("leaf");
        first.add_port("o", IO::Output(1));
        first.instantiate(&sink, Some("u"), None);
        first.set_usage(Usage::EmitStubAndStop);
        let second = ModDef::new("leaf");
        second.add_port("o", IO::Output(1));
        second.set_usage(Usage::EmitStubAndStop);
        let top = top_of(&[&first, &second]);
        let design = resolve_design(&top);
        assert!(design.diagnostics.is_empty(), "{:?}", design.diagnostics);
        assert!(design.collapsed(&second.core));
        assert_eq!(emitted_names(&top), ["leaf", "top"]);
    }

    #[test]
    fn resolve_design_terminates_on_cycles() {
        let a = ModDef::new("a");
        a.add_port("x", IO::Output(8));
        let b = a.wrap(Some("b"), Some("a_inst"));
        let b_inst = a.instantiate(&b, Some("b_inst"), None);
        b_inst.get_port("x").connect(&a.get_port("x"));
        resolve_design(&a);
    }
}
