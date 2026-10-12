// SPDX-License-Identifier: Apache-2.0

//! Typed emission: nets and ports declared with their driver's type, member
//! selects and indices for typed connections, the flat fallback, and the
//! build manifest. Every emitted design is elaborated with Slang together
//! with its sources.

use std::path::PathBuf;
use std::process::Command;

use slang_rs::str2tmpfile;
use tempfile::NamedTempFile;
use topstitch::*;

const PKG: &str = "
package cfg_pkg;
  typedef struct packed { logic valid; logic [3:0] dst_id; logic [7:0] len; } hdr_t;
  typedef struct packed { hdr_t hdr; logic [31:0] data; } flit_t;
  typedef struct packed { logic valid; logic [3:0] dst_id; logic [7:0] len; } hdr2_t;
  typedef struct packed { hdr2_t hdr; logic [31:0] data; } flit2_t;
  typedef struct packed { logic [7:0] len; logic [3:0] dst_id; logic valid; logic [31:0] data; } flat_t;
endpackage
";

const MODS: &str = "
module src import cfg_pkg::*; (output flit_t o, output flit_t [2:0] arr, output flit_t [5:4] off);
endmodule
module dst import cfg_pkg::*; (input flit2_t i, input hdr_t h, input logic v, input flit_t [2:0] a,
  input logic [31:0] d);
endmodule
module raw (input logic [44:0] x, input cfg_pkg::flat_t f);
endmodule
";

const CHILD: &str = "
module child #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output T out_o
);
endmodule
";

/// Sources written to temp files; kept alive while the design is in use.
struct Sources {
    files: Vec<NamedTempFile>,
}

impl Sources {
    fn new(sources: &[&str]) -> Self {
        Sources {
            files: sources.iter().map(|s| str2tmpfile(s).unwrap()).collect(),
        }
    }

    fn paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .map(|f| f.path().to_str().unwrap())
            .collect()
    }

    fn import(&self, name: &str) -> ModDef {
        let paths = self.paths();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    }

    /// Elaborates `emitted` with these sources under top `top`; returns
    /// Slang's output unless the build has no errors and no warnings.
    fn elaborate(&self, emitted: &str, top: &str) -> Result<(), String> {
        let rtl = str2tmpfile(emitted).unwrap();
        let slang = std::env::var("SLANG_PATH").unwrap_or_else(|_| "slang".into());
        let output = Command::new(slang)
            .args(self.paths())
            .arg(rtl.path())
            .args(["--top", top])
            .output()
            .expect("running Slang");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if output.status.success() && text.contains("Build succeeded: 0 errors, 0 warnings") {
            Ok(())
        } else {
            Err(text)
        }
    }

    /// Emits `top`, checks that Slang elaborates it, and returns the text.
    fn emit_checked(&self, top: &ModDef) -> String {
        let emitted = top.emit(EmitOptions::default());
        if let Err(slang) = self.elaborate(&emitted, &top.get_name()) {
            panic!("Slang rejects the emitted RTL:\n{emitted}\n{slang}");
        }
        emitted
    }
}

fn ty(value: &str) -> ParameterValue {
    ParameterValue::Type(value.to_string())
}

#[test]
fn two_type_specializations_emit() {
    let sources = Sources::new(&[PKG, CHILD]);
    let child = sources.import("child");
    let with_flit = child.parameterize(&[("T", ty("cfg_pkg::flit_t"))]);
    let with_hdr = child.parameterize(&[("T", ty("cfg_pkg::hdr_t"))]);

    let top = ModDef::new("top");
    let a = top.instantiate(&with_flit, Some("a"), None);
    let b = top.instantiate(&with_hdr, Some("b"), None);
    a.get_port("in_i").export_as("a_in");
    a.get_port("out_o")
        .field("hdr")
        .connect(&b.get_port("in_i").index(0));
    a.get_port("out_o").field("data").unused();
    b.get_port("out_o").connect(&b.get_port("in_i").index(1));
    b.get_port("out_o").export_as("b_out");

    assert_eq!(
        sources.emit_checked(&top),
        "\
module top(
  input cfg_pkg::flit_t [1:0] a_in,
  output cfg_pkg::hdr_t b_out
);
  wire cfg_pkg::flit_t a_out_o;
  child #(
    .T(cfg_pkg::flit_t)
  ) a (
    .in_i(a_in),
    .out_o({a_out_o.hdr, a_out_o.data})
  );
  child #(
    .T(cfg_pkg::hdr_t)
  ) b (
    .in_i({b_out, a_out_o.hdr}),
    .out_o(b_out)
  );
endmodule
"
    );
}

#[test]
fn field_and_index_connections_emit_member_selects() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // `flit_t` to `flit2_t`: equivalent layouts, so the net stays typed.
    s.get_port("o").connect(&d.get_port("i"));
    s.get_port("o").field("hdr").connect(&d.get_port("h"));
    s.get_port("arr").connect(&d.get_port("a"));
    s.get_port("arr")
        .index(1)
        .field("hdr.valid")
        .connect(&d.get_port("v"));
    // `[5:4]` is declared `[1:0]`: element 5 is index 1.
    s.get_port("off")
        .index(5)
        .field("data")
        .connect(&d.get_port("d"));
    s.get_port("off").index(5).field("hdr").unused();
    s.get_port("off").index(4).export_as("lo");

    assert_eq!(
        sources.emit_checked(&top),
        "\
module top(
  output cfg_pkg::flit_t lo
);
  wire cfg_pkg::flit_t s_o;
  wire cfg_pkg::flit_t [2:0] s_arr;
  wire cfg_pkg::flit_t [1:0] s_off;
  src s (
    .o(s_o),
    .arr(s_arr),
    .off({s_off[1].hdr, s_off[1].data, lo})
  );
  dst d (
    .i(s_o),
    .h(s_o.hdr),
    .v(s_arr[1].hdr.valid),
    .a(s_arr),
    .d(s_off[1].data)
  );
endmodule
"
    );
}

#[test]
fn mismatched_ends_fall_back_to_flat_nets() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst, raw) = (
        sources.import("src"),
        sources.import("dst"),
        sources.import("raw"),
    );
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    let r = top.instantiate(&raw, Some("r"), None);
    // `s.o` also reaches a plain vector, so its net is flat everywhere.
    s.get_port("o").connect(&d.get_port("i"));
    s.get_port("o").connect(&r.get_port("x"));
    s.get_port("o").field("hdr").connect(&d.get_port("h"));
    // `s.arr[0]` is reinterpreted with `connect_bits` as another layout.
    s.get_port("arr").connect(&d.get_port("a"));
    s.get_port("arr").index(0).connect_bits(&r.get_port("f"));
    // A plain bit slice of a typed port.
    s.get_port("off").slice(31, 0).connect(&d.get_port("d"));
    s.get_port("off").slice(89, 32).unused();
    d.get_port("v").tieoff(1);

    assert_eq!(
        sources.emit_checked(&top),
        "\
module top;
  wire [44:0] s_o;
  wire [134:0] s_arr;
  wire [89:0] s_off;
  src s (
    .o(s_o),
    .arr(s_arr),
    .off({s_off[89:32], s_off[31:0]})
  );
  dst d (
    .i(s_o),
    .h(s_o[44:32]),
    .v(1'h1),
    .a(s_arr),
    .d(s_off[31:0])
  );
  raw r (
    .x(s_o),
    .f(s_arr[44:0])
  );
endmodule
"
    );
}

#[test]
fn typed_ports_on_the_parent() {
    let sources = Sources::new(&[PKG, MODS]);
    let (src, dst) = (sources.import("src"), sources.import("dst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let d = top.instantiate(&dst, Some("d"), None);
    // Typed exports: a whole port and an element; a plain slice and a port
    // added without a type stay flat.
    let flit_in = d.get_port("i").export_as("flit_in");
    d.get_port("a").index(2).export_as("a2");
    d.get_port("a").slice(89, 0).export_as("a_lo");
    top.add_port("h_in", IO::Input(13))
        .connect(&d.get_port("h"));
    assert_eq!(flit_in.port_type(), dst.get_port("i").port_type());
    assert_eq!(
        top.get_port("a2").port_type().unwrap().to_string(),
        "cfg_pkg::flit_t"
    );
    assert_eq!(top.get_port("a_lo").port_type(), None);
    // Fields of a typed parent port feed instance ports and parent outputs.
    d.get_port("v").connect(&flit_in.field("hdr.valid"));
    d.get_port("d").connect(&flit_in.field("data"));
    flit_in.field("hdr").export_as("hdr_echo");
    // An instance output drives a typed parent output; the elements left
    // unused are member selects of the instance's typed net.
    s.get_port("o").export_as("o_out");
    s.get_port("arr").index(1).export_as("arr1");
    s.get_port("arr").index(0).unused();
    s.get_port("arr").index(2).unused();
    s.get_port("off").unused();

    assert_eq!(
        sources.emit_checked(&top),
        "\
module top(
  input cfg_pkg::flit2_t flit_in,
  input cfg_pkg::flit_t a2,
  input wire [89:0] a_lo,
  input wire [12:0] h_in,
  output cfg_pkg::hdr2_t hdr_echo,
  output cfg_pkg::flit_t o_out,
  output cfg_pkg::flit_t arr1
);
  wire cfg_pkg::flit_t [2:0] s_arr;
  src s (
    .o(o_out),
    .arr({s_arr[2], arr1, s_arr[0]}),
    .off()
  );
  dst d (
    .i(flit_in),
    .h(h_in),
    .v(flit_in.hdr.valid),
    .a({a2, a_lo}),
    .d(flit_in.data)
  );
  assign hdr_echo = flit_in.hdr;
endmodule
"
    );
}

#[test]
fn slang_check_rejects_flat_union_connection() {
    // The checker is not vacuous: a flat vector does not connect to an
    // unpacked union port.
    let sources = Sources::new(&[
        "package u_pkg; typedef union { logic [1:0] a; logic [2:0] b; } u_t; endpackage\n\
         module ua (output u_pkg::u_t x); endmodule\n\
         module ub (input u_pkg::u_t y); endmodule\n",
    ]);
    let flat = "module top;\n  wire [2:0] n;\n  ua a (.x(n));\n  ub b (.y(n));\nendmodule\n";
    let err = sources.elaborate(flat, "top").unwrap_err();
    assert!(err.contains("cannot be assigned"), "{err}");

    let (ua, ub) = (sources.import("ua"), sources.import("ub"));
    let top = ModDef::new("top");
    let a = top.instantiate(&ua, Some("a"), None);
    let b = top.instantiate(&ub, Some("b"), None);
    a.get_port("x").connect(&b.get_port("y"));
    assert!(
        sources
            .emit_checked(&top)
            .contains("  wire u_pkg::u_t a_x;\n")
    );
}

#[test]
fn manifest_lists_sources_and_specializations() {
    let pkg = Sources::new(&[PKG]);
    let mods = Sources::new(&[MODS]);
    let child_sv = Sources::new(&[CHILD]);
    let import = |files: &[&Sources], name: &str| {
        let paths: Vec<&str> = files.iter().flat_map(|s| s.paths()).collect();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    };
    let src = import(&[&pkg, &mods], "src");
    let child = import(&[&pkg, &child_sv], "child");
    let with_flit = child.parameterize(&[("T", ty("cfg_pkg::flit_t"))]);
    let with_hdr = child.parameterize(&[("T", ty("cfg_pkg::hdr_t"))]);

    let mid = ModDef::new("mid");
    mid.instantiate(&with_hdr, Some("c"), None);
    let top = ModDef::new("top");
    top.instantiate(&with_flit, Some("a"), None);
    top.instantiate(&src, Some("s"), None);
    top.instantiate(&mid, Some("m0"), None);
    top.instantiate(&mid, Some("m1"), None);

    let path = |s: &Sources| -> PathBuf { std::fs::canonicalize(s.files[0].path()).unwrap() };
    let manifest = top.manifest();
    assert_eq!(
        manifest.sources,
        vec![path(&pkg), path(&child_sv), path(&mods)]
    );
    assert!(manifest.sources.iter().all(|p| p.is_absolute()));
    let flit = with_flit.specialization_key().unwrap().short_hash();
    let hdr = with_hdr.specialization_key().unwrap().short_hash();
    assert_ne!(flit, hdr);
    assert_eq!(
        manifest.specializations,
        vec![
            ("top.a".to_string(), flit),
            ("top.m0.c".to_string(), hdr.clone()),
            ("top.m1.c".to_string(), hdr),
        ]
    );
    // Untyped, unimported designs have an empty manifest.
    assert_eq!(ModDef::new("empty").manifest(), Manifest::default());
}
