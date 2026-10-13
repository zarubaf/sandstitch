// SPDX-License-Identifier: Apache-2.0

//! Each error index example (`doc/errors/S0xxx.md`) triggers its code.
//!
//! The SystemVerilog blocks of an entry are written to files, one per block,
//! and its `cfg` lists them in order. The Rust statements run here are
//! exactly the entry's: [`documented!`] checks each against the entry's Rust
//! blocks (ignoring whitespace and `//` comments) before running it, and
//! [`Entry::finish`] checks that all of them ran. Where an entry shows the
//! rendered output (`text` block), its header, notes and helps must be those
//! of the run, and every source line it quotes must be in the example.

use std::cell::RefCell;
use std::path::PathBuf;

use tempfile::TempDir;
use topstitch::*;

/// An error index entry with its SystemVerilog blocks written to files.
struct Entry {
    code: Code,
    /// The Rust blocks without comments and whitespace.
    rust: String,
    /// The statements run so far, without whitespace.
    run: RefCell<String>,
    /// The rendered output shown (`text` blocks).
    output: Vec<String>,
    /// The trimmed lines of the SystemVerilog and Rust blocks.
    source_lines: Vec<String>,
    files: Vec<PathBuf>,
    _dir: TempDir,
}

/// The fenced blocks of `text` with info string `lang`.
fn blocks(text: &str, lang: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match &mut current {
            None if line == format!("```{lang}") => current = Some(String::new()),
            Some(block) if line == "```" => {
                out.push(std::mem::take(block));
                current = None;
            }
            Some(block) => {
                block.push_str(line);
                block.push('\n');
            }
            None => {}
        }
    }
    out
}

fn strip_whitespace(text: &str) -> String {
    text.split_whitespace().collect()
}

/// Rust code without `//` comments (the examples have no `//` in strings).
fn strip_comments(code: &str) -> String {
    code.lines()
        .map(|line| line.split("//").next().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The header, notes and helps of a rendered diagnostic.
#[derive(Debug, PartialEq)]
struct Summary {
    header: String,
    notes: Vec<String>,
    helps: Vec<String>,
}

fn summary(rendered: &str) -> Summary {
    let tagged = |tag: &str| -> Vec<String> {
        rendered
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix(tag))
            .map(str::to_string)
            .collect()
    };
    Summary {
        header: rendered.lines().next().unwrap_or_default().to_string(),
        notes: tagged("= note: "),
        helps: tagged("= help: "),
    }
}

/// The source lines a rendered diagnostic quotes (`12 | text`).
fn quoted_lines(rendered: &str) -> Vec<String> {
    rendered
        .lines()
        .filter_map(|l| {
            let (gutter, text) = l.split_once(" | ")?;
            let gutter = gutter.trim();
            (!gutter.is_empty() && gutter.bytes().all(|b| b.is_ascii_digit()))
                .then(|| text.trim().to_string())
        })
        .collect()
}

impl Entry {
    fn load(code: Code) -> Entry {
        let text = std::fs::read_to_string(format!("doc/errors/{}.md", code.0)).unwrap();
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let sv = blocks(&text, "systemverilog");
        let files = sv
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let path = dir.path().join(format!("block{i}.sv"));
                std::fs::write(&path, block).unwrap();
                path
            })
            .collect();
        let rust = blocks(&text, "rust");
        assert!(!rust.is_empty(), "{} has no Rust example", code.0);
        let source_lines = sv
            .iter()
            .chain(&rust)
            .flat_map(|block| block.lines().map(|l| l.trim().to_string()))
            .collect();
        Entry {
            code,
            rust: strip_whitespace(&strip_comments(&rust.concat())),
            run: RefCell::new(String::new()),
            output: blocks(&text, "text"),
            source_lines,
            files,
            _dir: dir,
        }
    }

    fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|p| p.to_str().unwrap()).collect()
    }

    /// Panics unless `code` (from `stringify!`) is the next documented
    /// statement.
    fn assert_documented(&self, code: &str) {
        let mut run = self.run.borrow_mut();
        run.push_str(&strip_whitespace(code));
        assert!(
            self.rust.starts_with(run.as_str()),
            "{}: statement not next in the documented example: {code}",
            self.code.0
        );
    }

    /// Panics unless every documented statement ran.
    fn finish(&self) {
        assert_eq!(
            *self.run.borrow(),
            self.rust,
            "{}: not every documented statement ran",
            self.code.0
        );
    }

    /// The single output block.
    fn shown(&self) -> &str {
        assert_eq!(self.output.len(), 1, "{}: one `text` block", self.code.0);
        &self.output[0]
    }

    /// Panics unless the output shown has the header, notes and helps of
    /// `rendered`, and quotes only lines of the example.
    fn assert_summary(&self, rendered: &str) {
        let shown = self.shown();
        assert_eq!(summary(shown), summary(rendered), "{}", self.code.0);
        for line in quoted_lines(shown) {
            assert!(
                self.source_lines.contains(&line),
                "{}: quoted line not in the example: {line}",
                self.code.0
            );
        }
    }

    /// Panics unless `rendered` is the entry's output.
    fn assert_output(&self, rendered: &str) {
        self.assert_summary(rendered);
        assert_eq!(self.shown(), format!("{rendered}\n"));
    }
}

/// Runs statements that the entry documents, checking each first.
macro_rules! documented {
    ($entry:expr;) => {};
    ($entry:expr; let $p:pat = $e:expr; $($rest:tt)*) => {
        $entry.assert_documented(stringify!(let $p = $e;));
        let $p = $e;
        documented!($entry; $($rest)*);
    };
    ($entry:expr; $e:expr; $($rest:tt)*) => {
        $entry.assert_documented(stringify!($e;));
        $e;
        documented!($entry; $($rest)*);
    };
}

/// The message `f` panics with.
fn panic_message(f: impl FnOnce()) -> String {
    let payload =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("expected a panic");
    *payload.downcast::<String>().expect("a String panic")
}

fn assert_code(err: &ImportError, code: Code, message: &str) {
    let first = &err.diagnostics.0[0];
    assert_eq!(first.code, code, "{}", err.diagnostics.render());
    assert_eq!(first.message, message);
}

/// `cfg` listing the entry's sources.
fn config<'a>(paths: &'a [&'a str]) -> ParserConfig<'a> {
    ParserConfig {
        sources: paths,
        ..Default::default()
    }
}

#[test]
fn s0101() {
    let entry = Entry::load(diagnostic::codes::S0101);
    let paths = entry.paths();
    let cfg = config(&paths);
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            let router = ModDef::try_from_verilog_with_config("router", &cfg)?;
            router.try_parameterize(&[("flit_tt", ParameterValue::Type("cfg_pkg::flit_t".into()))])?;
        }
        Ok(())
    };
    assert_code(
        &run().unwrap_err(),
        diagnostic::codes::S0101,
        "`router` has no overridable parameter `flit_tt`",
    );
    entry.finish();
}

#[test]
fn s0102() {
    let entry = Entry::load(diagnostic::codes::S0102);
    let paths = entry.paths();
    let cfg = config(&paths);
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            ModDef::try_from_verilog_with_config("router", &cfg)?;
        }
        Ok(())
    };
    let err = run().unwrap_err();
    assert_code(&err, diagnostic::codes::S0102, "cannot elaborate `router`");
    let label = &err.diagnostics.0[0].primary.text;
    assert!(label.contains("cfg_pkg"), "{label}");
    entry.finish();
}

#[test]
fn s0103() {
    let entry = Entry::load(diagnostic::codes::S0103);
    let paths = entry.paths();
    let cfg = config(&paths);
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            ModDef::try_from_verilog_with_config("m", &cfg)?;
        }
        Ok(())
    };
    assert_code(
        &run().unwrap_err(),
        diagnostic::codes::S0103,
        "port `mem` of `m` has unpacked dimensions",
    );
    entry.finish();
}

#[test]
fn s0104() {
    let entry = Entry::load(diagnostic::codes::S0104);
    let paths = entry.paths();
    let cfg = config(&paths);
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            ModDef::try_from_verilog_with_config("m", &cfg)?;
        }
        Ok(())
    };
    assert_code(
        &run().unwrap_err(),
        diagnostic::codes::S0104,
        "port `bus` of `m` is an interface port",
    );
    entry.finish();
}

#[test]
fn s0105() {
    let entry = Entry::load(diagnostic::codes::S0105);
    let paths = entry.paths();
    let cfg = config(&paths);
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            let router = ModDef::try_from_verilog_with_config("router", &cfg)?;
            router.try_parameterize(&[("flit_t", ParameterValue::Type("struct packed { logic a; }".into()))])?;
        }
        Ok(())
    };
    assert_code(
        &run().unwrap_err(),
        diagnostic::codes::S0105,
        "unsupported override for parameter `flit_t` of `router`",
    );
    entry.finish();
}

#[test]
fn s0201() {
    let entry = Entry::load(diagnostic::codes::S0201);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let producer = ModDef::from_verilog_with_config("producer", &cfg);
        let consumer = ModDef::from_verilog_with_config("consumer", &cfg);
        let top = ModDef::new("top");
        let p = top.instantiate(&producer, Some("p"), None);
        let c = top.instantiate(&consumer, Some("c"), None);
        p.get_port("req_o").connect(&c.get_port("req_i"));
        let diagnostics = top.check();
    }
    entry.finish();
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    assert_eq!(diagnostics.0[0].code, diagnostic::codes::S0201);
    entry.assert_summary(&diagnostics.render());
}

/// The examples that select from `sel` and panic.
fn sel_panic(code: Code, select: impl FnOnce(&Entry, &ModDef)) {
    let entry = Entry::load(code);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let sel = ModDef::from_verilog_with_config("sel", &cfg);
    }
    let rendered = panic_message(|| select(&entry, &sel));
    entry.finish();
    assert!(
        rendered.starts_with(&format!("error[{}]: ", code.0)),
        "{rendered}"
    );
    entry.assert_summary(&rendered);
}

#[test]
fn s0202() {
    sel_panic(diagnostic::codes::S0202, |entry, sel| {
        documented! { entry;
            sel.get_port("hdrs_i").index(3).field("dst_idd");
        }
    });
}

#[test]
fn s0203() {
    sel_panic(diagnostic::codes::S0203, |entry, sel| {
        documented! { entry;
            sel.get_port("hdrs_i").index(8);
        }
    });
}

#[test]
fn s0204() {
    sel_panic(diagnostic::codes::S0204, |entry, sel| {
        documented! { entry;
            sel.get_port("u_i").field("hdr.dst_id");
        }
    });
}

#[test]
fn s0205() {
    sel_panic(diagnostic::codes::S0205, |entry, sel| {
        documented! { entry;
            sel.get_port("hdr_i").field("len");
        }
    });
}

/// The examples that define interfaces on `node` and panic.
fn node_panic(code: Code, run: impl FnOnce(&Entry, &ModDef)) {
    let entry = Entry::load(code);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let node = ModDef::from_verilog_with_config("node", &cfg);
    }
    let rendered = panic_message(|| run(&entry, &node));
    entry.finish();
    assert!(
        rendered.starts_with(&format!("error[{}]: ", code.0)),
        "{rendered}"
    );
    entry.assert_summary(&rendered);
}

#[test]
fn s0207() {
    node_panic(diagnostic::codes::S0207, |entry, node| {
        // The example owns `node`.
        let node = node.clone();
        documented! { entry;
            node.def_intf_members("link", [("req", node.get_port("req_o").index(0)), ("req", node.get_port("req_i").index(0)), ("rsp", node.get_port("rsp_o").index(0))]);
            let top = ModDef::new("top");
            let a = top.instantiate(&node, Some("a"), None);
            let b = top.instantiate(&node, Some("b"), None);
            a.get_intf("link").connect(&b.get_intf("link"), false);
        }
    });
}

#[test]
fn s0208() {
    node_panic(diagnostic::codes::S0208, |entry, node| {
        documented! { entry;
            node.def_intf_members("link", [("req", node.get_port("req_o").index(0)), ("req", node.get_port("req_o").index(1))]);
        }
    });
}

#[test]
fn s0209() {
    node_panic(diagnostic::codes::S0209, |entry, node| {
        documented! { entry;
            let link = node.def_intf_members("link", [("req", node.get_port("req_o").index(0)), ("req", node.get_port("req_i").index(0))]);
            link.get("req");
        }
    });
}

#[test]
fn s0206() {
    let entry = Entry::load(diagnostic::codes::S0206);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let sel = ModDef::from_verilog_with_config("sel", &cfg);
        let top = ModDef::new("top");
        top.instantiate(&sel, Some("s"), None).get_port("hdr_i").tieoff(0);
        let diagnostics = top.check();
    }
    entry.finish();
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    assert_eq!(diagnostics.0[0].code, diagnostic::codes::S0206);
    // The example shows the file as `sel.sv`.
    entry.assert_output(&diagnostics.render().replace(paths[0], "sel.sv"));
}

#[test]
fn s0301() {
    let entry = Entry::load(diagnostic::codes::S0301);
    let paths = entry.paths();
    assert_eq!(paths.len(), 1, "the example is one file");
    let cfg = config(&paths);
    let message = panic_message(|| {
        documented! { entry;
            let producer = ModDef::from_verilog_with_config("producer", &cfg);
            let consumer = ModDef::from_verilog_with_config("consumer", &cfg);
            consumer.set_usage(Usage::EmitStubAndStop);
            let top = ModDef::new("top");
            let p = top.instantiate(&producer, Some("p"), None);
            let c = top.instantiate(&consumer, Some("c"), None);
            p.get_port("data_o").connect(&c.get_port("data_i"));
            top.manifest();
        }
    });
    entry.finish();
    // The example shows the file as `/work/blocks.sv`.
    entry.assert_output(&message.replace(paths[0], "/work/blocks.sv"));
}

#[test]
fn s0302() {
    let entry = Entry::load(diagnostic::codes::S0302);
    let paths = entry.paths();
    let [narrow_sv, wide_sv] = paths[..] else {
        panic!("the example has two files: {paths:?}");
    };
    let message = panic_message(|| {
        documented! { entry;
            let narrow = ModDef::from_verilog_with_config("narrow", &ParserConfig {
                sources: &[narrow_sv],
                defines: &[("WIDTH", "8")],
                ..Default::default()
            });
            let wide = ModDef::from_verilog_with_config("wide", &ParserConfig {
                sources: &[wide_sv],
                defines: &[("WIDTH", "16")],
                ..Default::default()
            });
            let top = ModDef::new("top");
            top.instantiate(&narrow, Some("n"), None);
            top.instantiate(&wide, Some("w"), None);
            top.manifest();
        }
    });
    entry.finish();
    entry.assert_output(&message);
}

/// Asserts that `diagnostics` is one diagnostic of the entry's code about
/// `object` (definition, end, msb, lsb), and that the entry shows it.
fn assert_connection_error(
    entry: &Entry,
    diagnostics: &Diagnostics,
    (definition, end, msb, lsb): (&str, &str, usize, usize),
) {
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    let d = &diagnostics.0[0];
    assert_eq!(d.code, entry.code, "{}", diagnostics.render());
    let object = ObjectRef {
        definition: definition.into(),
        end: end.into(),
        msb,
        lsb,
    };
    assert_eq!(d.object, Some(object));
    entry.assert_summary(&diagnostics.render());
}

#[test]
fn s0401() {
    let entry = Entry::load(diagnostic::codes::S0401);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let src = ModDef::from_verilog_with_config("src", &cfg);
        let snk = ModDef::from_verilog_with_config("snk", &cfg);
        let top = ModDef::new("top");
        let u = top.instantiate(&snk, Some("u"), None);
        let a = top.instantiate(&src, Some("a"), None);
        let b = top.instantiate(&src, Some("b"), None);
        a.get_port("o").connect(&u.get_port("i"));
        b.get_port("o").connect(&u.get_port("i"));
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "u.i", 7, 0));
}

#[test]
fn s0402() {
    let entry = Entry::load(diagnostic::codes::S0402);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let snk = ModDef::from_verilog_with_config("snk", &cfg);
        let top = ModDef::new("top");
        let u = top.instantiate(&snk, Some("u"), None);
        u.get_port("i").slice(7, 4).tieoff(0);
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "u.i", 3, 0));
}

#[test]
fn s0403() {
    let entry = Entry::load(diagnostic::codes::S0403);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let snk = ModDef::from_verilog_with_config("snk", &cfg);
        let top = ModDef::new("top");
        let u = top.instantiate(&snk, Some("u"), None);
        u.get_port("i").specify_net_name("data");
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "u.i", 7, 0));
}

#[test]
fn s0404() {
    let entry = Entry::load(diagnostic::codes::S0404);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let src = ModDef::from_verilog_with_config("src", &cfg);
        let snk = ModDef::from_verilog_with_config("snk", &cfg);
        let top = ModDef::new("top");
        let a = top.instantiate(&src, Some("a"), None);
        let u = top.instantiate(&snk, Some("u"), None);
        a.get_port("o").connect(&u.get_port("i"));
        a.get_port("o").slice(3, 0).unused();
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "a.o", 3, 0));
}

#[test]
fn s0405() {
    let entry = Entry::load(diagnostic::codes::S0405);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let src = ModDef::from_verilog_with_config("src", &cfg);
        let top = ModDef::new("top");
        let a = top.instantiate(&src, Some("a"), None);
        a.get_port("o").tieoff(0);
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "a.o", 7, 0));
}

#[test]
fn s0406() {
    let entry = Entry::load(diagnostic::codes::S0406);
    documented! { entry;
        let top = ModDef::new("top");
        let p = top.add_port("p", IO::InOut(8));
        let q = top.add_port("q", IO::InOut(8));
        p.connect(&q);
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "p", 7, 0));
}

#[test]
fn s0407() {
    let entry = Entry::load(diagnostic::codes::S0407);
    let paths = entry.paths();
    let cfg = config(&paths);
    documented! { entry;
        let src = ModDef::from_verilog_with_config("src", &cfg);
        let snk = ModDef::from_verilog_with_config("snk", &cfg);
        let top = ModDef::new("top");
        top.add_port("data", IO::Input(8)).unused();
        let a = top.instantiate(&src, Some("a"), None);
        let u = top.instantiate(&snk, Some("u"), None);
        a.get_port("o").connect(&u.get_port("i"));
        a.get_port("o").specify_net_name("data");
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "a.o", 7, 0));
}

#[test]
fn s0409() {
    let entry = Entry::load(diagnostic::codes::S0409);
    documented! { entry;
        let top = ModDef::new("top");
        let i = top.add_port("i", IO::Input(8));
        let o = top.add_port("o", IO::Output(8));
        o.connect(&i);
        o.slice(3, 0).connect(&i.slice(3, 0));
        let diagnostics = top.check();
    }
    entry.finish();
    assert_connection_error(&entry, &diagnostics, ("top", "i", 3, 0));
}

#[test]
fn every_code_has_an_example_test() {
    let source = include_str!("examples.rs");
    for code in diagnostic::codes::ALL {
        let name = code.0.to_lowercase();
        assert!(
            source.contains(&format!("#[test]\nfn {name}() {{")),
            "no example test for {}",
            code.0
        );
    }
}
