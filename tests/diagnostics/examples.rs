// SPDX-License-Identifier: Apache-2.0

//! Each error index example (`doc/errors/S0xxx.md`) triggers its code.
//!
//! The SystemVerilog blocks of an entry are written to files, one per block,
//! and its `cfg` lists them in order. The Rust statements run here are the
//! entry's: [`documented!`] checks that each appears in the entry's Rust
//! blocks (ignoring whitespace) before running it.

use std::path::PathBuf;

use tempfile::TempDir;
use topstitch::*;

/// An error index entry with its SystemVerilog blocks written to files.
struct Entry {
    rust: String,
    /// The rendered output shown (`text` blocks).
    output: Vec<String>,
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

impl Entry {
    fn load(code: Code) -> Entry {
        let text = std::fs::read_to_string(format!("doc/errors/{}.md", code.0)).unwrap();
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let files = blocks(&text, "systemverilog")
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let path = dir.path().join(format!("block{i}.sv"));
                std::fs::write(&path, block).unwrap();
                path
            })
            .collect();
        let rust: String = blocks(&text, "rust").concat();
        assert!(!rust.is_empty(), "{} has no Rust example", code.0);
        Entry {
            rust: rust.split_whitespace().collect(),
            output: blocks(&text, "text"),
            files,
            _dir: dir,
        }
    }

    /// Panics unless `rendered` is the entry's output.
    fn assert_output(&self, rendered: &str) {
        assert_eq!(self.output, [format!("{rendered}\n")]);
    }

    fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|p| p.to_str().unwrap()).collect()
    }

    /// Panics unless `code` (from `stringify!`) appears in the Rust blocks.
    fn assert_documented(&self, code: &str) {
        let code: String = code.split_whitespace().collect();
        assert!(
            self.rust.contains(&code),
            "statement not in the documented example: {code}"
        );
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

/// `rendered` starts with the header of `code` and `message`.
fn assert_rendered(rendered: &str, code: Code, message: &str) {
    let header = format!("error[{}]: {message}\n", code.0);
    assert!(rendered.starts_with(&header), "{rendered}");
}

#[test]
fn s0101() {
    let entry = Entry::load(diagnostic::codes::S0101);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
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
}

#[test]
fn s0102() {
    let entry = Entry::load(diagnostic::codes::S0102);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    let run = || -> Result<(), ImportError> {
        documented! { entry;
            ModDef::try_from_verilog_with_config("router", &cfg)?;
        }
        Ok(())
    };
    let err = run().unwrap_err();
    assert_code(
        &err,
        diagnostic::codes::S0102,
        "Slang failed to elaborate `router`",
    );
    let label = &err.diagnostics.0[0].primary.text;
    assert!(label.contains("cfg_pkg"), "{label}");
}

#[test]
fn s0103() {
    let entry = Entry::load(diagnostic::codes::S0103);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
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
}

#[test]
fn s0104() {
    let entry = Entry::load(diagnostic::codes::S0104);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
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
}

#[test]
fn s0105() {
    let entry = Entry::load(diagnostic::codes::S0105);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
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
}

#[test]
fn s0201() {
    let entry = Entry::load(diagnostic::codes::S0201);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    documented! { entry;
        let producer = ModDef::from_verilog_with_config("producer", &cfg);
        let consumer = ModDef::from_verilog_with_config("consumer", &cfg);
        let top = ModDef::new("top");
        let p = top.instantiate(&producer, Some("p"), None);
        let c = top.instantiate(&consumer, Some("c"), None);
        p.get_port("req_o").connect(&c.get_port("req_i"));
        let diagnostics = top.check();
    }
    assert_eq!(diagnostics.0.len(), 1, "{}", diagnostics.render());
    assert_eq!(diagnostics.0[0].code, diagnostic::codes::S0201);
}

/// The examples that select from `sel` and panic.
fn sel_panic(code: Code, message: &str, select: impl FnOnce(&Entry, &ModDef)) {
    let entry = Entry::load(code);
    let paths = entry.paths();
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    documented! { entry;
        let sel = ModDef::from_verilog_with_config("sel", &cfg);
    }
    assert_rendered(&panic_message(|| select(&entry, &sel)), code, message);
}

#[test]
fn s0202() {
    sel_panic(
        diagnostic::codes::S0202,
        "no field `dst_idd` in `sel_pkg::hdr_t`",
        |entry, sel| {
            documented! { entry;
                sel.get_port("hdrs_i").index(3).field("dst_idd");
            }
        },
    );
}

#[test]
fn s0203() {
    sel_panic(
        diagnostic::codes::S0203,
        "index 8 is out of range for `sel_pkg::hdr_t [7:0]`",
        |entry, sel| {
            documented! { entry;
                sel.get_port("hdrs_i").index(8);
            }
        },
    );
}

#[test]
fn s0204() {
    sel_panic(
        diagnostic::codes::S0204,
        "cannot select field `hdr` through union `sel_pkg::hdr_u`",
        |entry, sel| {
            documented! { entry;
                sel.get_port("u_i").field("hdr.dst_id");
            }
        },
    );
}

#[test]
fn s0301() {
    let entry = Entry::load(diagnostic::codes::S0301);
    let paths = entry.paths();
    assert_eq!(paths.len(), 1, "the example is one file");
    let cfg = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
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
    entry.assert_output(&message);
}

/// The outputs shown for the typed connection codes are the snapshots of
/// `tests/diagnostics/typed.rs`, which renders them from its own call sites.
#[test]
fn typed_outputs_are_the_snapshots() {
    for (code, snapshot) in [
        (diagnostic::codes::S0201, "s0201_field_mismatch"),
        (diagnostic::codes::S0202, "s0202_no_such_field"),
        (diagnostic::codes::S0203, "s0203_index_out_of_range"),
        (diagnostic::codes::S0204, "s0204_field_through_union"),
    ] {
        let path = format!("tests/diagnostics/snapshots/{snapshot}.txt");
        let expected = std::fs::read_to_string(path).unwrap();
        assert_eq!(Entry::load(code).output, [expected], "{}", code.0);
    }
}

#[test]
fn every_code_has_an_example_test() {
    let source = include_str!("examples.rs");
    for code in diagnostic::codes::ALL {
        let name = code.0.to_lowercase();
        assert!(
            source.contains(&format!("fn {name}()")),
            "no example test for {}",
            code.0
        );
    }
}
