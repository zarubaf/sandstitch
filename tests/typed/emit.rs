// SPDX-License-Identifier: Apache-2.0

//! Typed emission: nets and ports declared with their driver's type, member
//! selects and indices for typed connections, the flat fallback, and the
//! build manifest. Every emitted design is elaborated with Slang together
//! with its sources, with width warnings enabled and no warning allowed;
//! typed selections are also checked bit by bit against hand-computed
//! layouts.

use std::path::{Path, PathBuf};
use std::process::Command;

use num_bigint::BigInt;
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
  typedef flit_t [1:0] flit_pair_t;
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
module psrc import cfg_pkg::*; (output flit_pair_t pair, output flit_t [0:2] asc);
endmodule
module pdst import cfg_pkg::*; (input flit_t f, input flit2_t g, input hdr_t h);
endmodule
";

const CHILD: &str = "
module child #(parameter type T = logic, parameter int N = 2) (
  input T [N-1:0] in_i,
  output T out_o
);
endmodule
";

/// Elaborates `files` plus `emitted` under `top` with width and multiple
/// driver warnings on; returns Slang's output unless the build has no errors
/// and no warnings. (`-Wpacked-array-conv` stays off: flat nets and
/// concatenations connect to packed-array ports by implicit conversion by
/// design.)
fn elaborate(files: &[&Path], emitted: &str, top: &str) -> Result<(), String> {
    let rtl = str2tmpfile(emitted).unwrap();
    let slang = std::env::var("SLANG_PATH").unwrap_or_else(|_| "slang".into());
    let output = Command::new(slang)
        .args(files)
        .arg(rtl.path())
        .args(["--top", top])
        .args([
            "-Wport-width-trunc",
            "-Wport-width-expand",
            "-Wwidth-trunc",
            "-Wwidth-expand",
            "-Wmultiple-cont-assigns",
        ])
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

    fn elaborate(&self, emitted: &str, top: &str) -> Result<(), String> {
        let files: Vec<&Path> = self.files.iter().map(|f| f.path()).collect();
        elaborate(&files, emitted, top)
    }

    /// Emits `top` with `options`, checks that Slang elaborates it, and
    /// returns the text.
    fn emit_checked_with(&self, top: &ModDef, options: EmitOptions) -> String {
        let emitted = top.emit(options);
        if let Err(slang) = self.elaborate(&emitted, &top.get_name()) {
            panic!("Slang rejects the emitted RTL:\n{emitted}\n{slang}");
        }
        emitted
    }

    fn emit_checked(&self, top: &ModDef) -> String {
        self.emit_checked_with(top, EmitOptions::default())
    }

    /// Checks with Slang that each `(select, msb, lsb)` applied to a value
    /// of type `ty` reads exactly bits `[msb:lsb]` of it: a value with only
    /// bit `lsb` (resp. `msb`) set must read as the selection's lowest
    /// (resp. highest) bit, and the selection must be `msb - lsb + 1` bits.
    fn check_bits(&self, ty: &str, selects: &[(&str, usize, usize)]) -> Result<(), String> {
        let mut body =
            format!("module bitchk;\n  typedef {ty} t;\n  localparam int W = $bits(t);\n");
        for (i, (select, msb, lsb)) in selects.iter().enumerate() {
            let n = msb - lsb + 1;
            body += &format!(
                "  localparam t L{i} = t'(W'(1) << {lsb});\n\
                 \x20 localparam t M{i} = t'(W'(1) << {msb});\n\
                 \x20 if ($bits(L{i}{select}) != {n}) $error(\"width of {select}\");\n\
                 \x20 if ({{L{i}{select}}} != {n}'(1)) $error(\"lsb of {select}\");\n\
                 \x20 if ({{M{i}{select}}} != ({n}'(1) << {})) $error(\"msb of {select}\");\n",
                n - 1
            );
        }
        body += "endmodule\n";
        self.elaborate(&body, "bitchk")
    }

    fn assert_bits(&self, ty: &str, selects: &[(&str, usize, usize)]) {
        self.check_bits(ty, selects)
            .unwrap_or_else(|e| panic!("{e}"));
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
    // flit_t: hdr [44:32], data [31:0].
    sources.assert_bits("cfg_pkg::flit_t", &[(".hdr", 44, 32), (".data", 31, 0)]);
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
    // `[5:4]` is declared as such; element 5 is the upper one.
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
  wire cfg_pkg::flit_t [5:4] s_off;
  src s (
    .o(s_o),
    .arr(s_arr),
    .off({s_off[5].hdr, s_off[5].data, lo})
  );
  dst d (
    .i(s_o),
    .h(s_o.hdr),
    .v(s_arr[1].hdr.valid),
    .a(s_arr),
    .d(s_off[5].data)
  );
endmodule
"
    );
    // Elements are 45 bits: element 1 of `[2:0]` is [89:45], its
    // `hdr.valid` bit 89; element 5 of `[5:4]` is [89:45], its `data`
    // [76:45], its `hdr` [89:77].
    sources.assert_bits("cfg_pkg::flit_t [2:0]", &[("[1].hdr.valid", 89, 89)]);
    sources.assert_bits(
        "cfg_pkg::flit_t [5:4]",
        &[("[5].data", 76, 45), ("[5].hdr", 89, 77), ("[4]", 44, 0)],
    );
}

#[test]
fn bit_check_is_not_vacuous() {
    let sources = Sources::new(&[PKG]);
    let err = sources
        .check_bits("cfg_pkg::flit_t [5:4]", &[("[5].data", 76, 46)])
        .unwrap_err();
    assert!(err.contains("lsb of [5].data"), "{err}");
    let err = sources
        .check_bits("cfg_pkg::flit_t [5:4]", &[("[4]", 89, 45)])
        .unwrap_err();
    assert!(err.contains("lsb of [4]"), "{err}");
}

#[test]
fn array_typedef_ports_use_their_element_type() {
    let sources = Sources::new(&[PKG, MODS]);
    let (psrc, pdst) = (sources.import("psrc"), sources.import("pdst"));
    // `flit_pair_t` is `flit_t [1:0]`: the name is the element's.
    assert_eq!(
        psrc.get_port("pair").port_type().unwrap().to_string(),
        "cfg_pkg::flit_t [1:0]"
    );
    assert_eq!(psrc.get_port("pair").io().width(), 90);

    let top = ModDef::new("top");
    let s = top.instantiate(&psrc, Some("s"), None);
    let d = top.instantiate(&pdst, Some("d"), None);
    s.get_port("pair").index(1).connect(&d.get_port("f"));
    s.get_port("pair").index(0).connect(&d.get_port("g"));
    s.get_port("pair")
        .index(0)
        .field("hdr")
        .connect(&d.get_port("h"));
    s.get_port("asc").unused();
    assert_eq!(
        sources.emit_checked(&top),
        "\
module top;
  wire cfg_pkg::flit_t [1:0] s_pair;
  psrc s (
    .pair(s_pair),
    .asc()
  );
  pdst d (
    .f(s_pair[1]),
    .g(s_pair[0]),
    .h(s_pair[0].hdr)
  );
endmodule
"
    );
    sources.assert_bits(
        "cfg_pkg::flit_pair_t",
        &[("[1]", 89, 45), ("[0]", 44, 0), ("[0].hdr", 44, 32)],
    );

    // Exported, the port keeps the array type.
    let top = ModDef::new("top");
    let s = top.instantiate(&psrc, Some("s"), None);
    s.get_port("pair").export_as("pair_out");
    s.get_port("asc").unused();
    assert_eq!(
        sources.emit_checked(&top),
        "\
module top(
  output cfg_pkg::flit_t [1:0] pair_out
);
  psrc s (
    .pair(pair_out),
    .asc()
  );
endmodule
"
    );
}

#[test]
fn exported_port_keeps_its_declared_range() {
    let sources = Sources::new(&[PKG, MODS]);
    let (psrc, pdst) = (sources.import("psrc"), sources.import("pdst"));
    let top = ModDef::new("top");
    let s = top.instantiate(&psrc, Some("s"), None);
    let d = top.instantiate(&pdst, Some("d"), None);
    let asc = s.get_port("asc").export_as("asc_out");
    assert_eq!(
        asc.port_type().unwrap().to_string(),
        "cfg_pkg::flit_t [0:2]"
    );
    // Ascending: element 0 is the most significant.
    s.get_port("asc").index(0).connect(&d.get_port("f"));
    s.get_port("asc")
        .index(2)
        .field("hdr")
        .connect(&d.get_port("h"));
    d.get_port("g").tieoff(0);
    s.get_port("pair").unused();
    assert_eq!(
        sources.emit_checked(&top),
        "\
module top(
  output cfg_pkg::flit_t [0:2] asc_out
);
  psrc s (
    .pair(),
    .asc(asc_out)
  );
  pdst d (
    .f(asc_out[0]),
    .g(45'h0000_0000_0000),
    .h(asc_out[2].hdr)
  );
endmodule
"
    );
    sources.assert_bits(
        "cfg_pkg::flit_t [0:2]",
        &[("[0]", 134, 90), ("[2]", 44, 0), ("[2].hdr", 44, 32)],
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
    sources.assert_bits(
        "cfg_pkg::flit2_t",
        &[(".hdr.valid", 44, 44), (".data", 31, 0), (".hdr", 44, 32)],
    );
    sources.assert_bits("cfg_pkg::flit_t [2:0]", &[("[2]", 134, 90), ("[0]", 44, 0)]);
}

/// A module whose ports are typed but whose definition is emitted with
/// tieoffs, so that the constants can be read back after elaboration.
const TIED: &str = "
module tied import cfg_pkg::*; (output flit_t o, output flit_t [0:2] asc);
endmodule
";

#[test]
fn tied_off_typed_ports_are_flat_and_bit_exact() {
    // Imported with its sources, but emitted from the model: only the
    // package is elaborated with the emitted definition.
    let pkg = Sources::new(&[PKG]);
    let module = Sources::new(&[TIED]);
    let tied = {
        let paths: Vec<&str> = pkg.paths().into_iter().chain(module.paths()).collect();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config("tied", &cfg)
    };
    tied.set_usage(Usage::EmitDefinitionAndDescend);
    let o = tied.get_port("o");
    o.field("hdr.valid").tieoff(1);
    o.field("hdr.dst_id").tieoff(0b1010);
    o.field("hdr.len").tieoff(0);
    o.field("data").tieoff(0x8000_0001u32);
    // One-hot elements: element 0 (the most significant) gets 1 << 44.
    let asc = tied.get_port("asc");
    asc.index(0).tieoff(BigInt::from(1u64) << 44);
    asc.index(1).tieoff(1);
    asc.index(2).tieoff(0);

    // Adjacent tieoffs merge. By hand: `o` is valid (bit 44) | dst_id 0xa
    // at [43:40] | data; `asc[0]` is bits [134:90], so its 1 << 44 is bit
    // 134, and `asc[1]`'s 1 is bit 45.
    for (bitblast, expected) in [
        (
            false,
            "\
module tied(
  output wire [44:0] o,
  output wire [134:0] asc
);
  assign o = 45'h1a00_8000_0001;
  assign asc = 135'h40_0000_0000_0000_0000_0000_2000_0000_0000;
endmodule
",
        ),
        (true, ""),
    ] {
        let emitted = pkg.emit_checked_with(
            &tied,
            EmitOptions {
                bitblast_assignments: bitblast,
                ..Default::default()
            },
        );
        if !bitblast {
            assert_eq!(emitted, expected);
        } else {
            assert_eq!(emitted.matches("  assign o[").count(), 45);
            assert!(emitted.contains("  assign o[44] = 1'h1;\n"), "{emitted}");
            assert!(emitted.contains("  output wire [44:0] o,\n"), "{emitted}");
        }
    }
    // The bit ranges the tieoffs were emitted to are the declared fields and
    // elements: element 0 of the ascending `[0:2]` is the most significant.
    pkg.assert_bits(
        "cfg_pkg::flit_t [0:2]",
        &[("[0]", 134, 90), ("[1]", 89, 45), ("[2]", 44, 0)],
    );
    pkg.assert_bits(
        "cfg_pkg::flit_t",
        &[
            (".hdr.valid", 44, 44),
            (".hdr.dst_id", 43, 40),
            (".data", 31, 0),
        ],
    );
}

#[test]
fn unpacked_union_tieoff_stays_flat() {
    let pkg = "package u_pkg; typedef union { logic [1:0] a; logic [2:0] b; } u_t; endpackage\n";
    let pkg_only = Sources::new(&[pkg]);
    let sources = Sources::new(&[
        pkg,
        "module ua (output u_pkg::u_t x); endmodule\nmodule ub (input u_pkg::u_t y); endmodule\n",
    ]);
    let ua = sources.import("ua");
    ua.set_usage(Usage::EmitDefinitionAndDescend);
    ua.get_port("x").tieoff(0);
    assert_eq!(
        pkg_only.emit_checked(&ua),
        "\
module ua(
  output wire [2:0] x
);
  assign x = 3'h0;
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
    // A source given by a relative path is listed as an absolute one.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    std::fs::write(
        dir.path().join("rel.sv"),
        "module rel (input logic a);\nendmodule\n",
    )
    .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let relative = dir.path().join("rel.sv");
    let relative = relative
        .strip_prefix(&cwd)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(Path::new(&relative).is_relative());

    let import = |paths: &[&str], name: &str| {
        let cfg = ParserConfig {
            sources: paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    };
    let (p, m, c) = (pkg.paths()[0], mods.paths()[0], child_sv.paths()[0]);
    let src = import(&[p, m], "src");
    let raw = import(&[p, m], "raw");
    let child = import(&[p, c], "child");
    let rel = import(&[&relative], "rel");
    let with_flit = child.parameterize(&[("T", ty("cfg_pkg::flit_t"))]);
    let with_hdr = child.parameterize(&[("T", ty("cfg_pkg::hdr_t"))]);
    // Inline text lives in a deleted temp file; it is not listed.
    let inline = ModDef::from_verilog(
        "inl",
        "module inl (input logic a);\nendmodule\n",
        false,
        false,
    );
    // `raw` is emitted from the model, so its sources are not listed
    // (here they are listed anyway, for `src`).
    raw.set_usage(Usage::EmitStubAndStop);

    let mid = ModDef::new("mid");
    let c0 = mid.instantiate(&with_hdr, Some("c"), None);
    c0.get_port("in_i").export_as("in_i");
    c0.get_port("out_o").export_as("out_o");
    let top = ModDef::new("top");
    let a = top.instantiate(&with_flit, Some("a"), None);
    let s = top.instantiate(&src, Some("s"), None);
    let m0 = top.instantiate(&mid, Some("m0"), None);
    let m1 = top.instantiate(&mid, Some("m1"), None);
    let r = top.instantiate(&raw, Some("r"), None);
    let l = top.instantiate(&rel, Some("l"), None);
    let i = top.instantiate(&inline, Some("i"), None);
    a.get_port("in_i").tieoff(0);
    a.get_port("out_o").unused();
    for port in ["o", "arr", "off"] {
        s.get_port(port).unused();
    }
    m0.get_port("out_o").connect(&m1.get_port("in_i").index(0));
    m0.get_port("in_i").tieoff(0);
    m1.get_port("in_i").index(1).tieoff(0);
    m1.get_port("out_o").unused();
    r.get_port("x").tieoff(0);
    r.get_port("f").tieoff(0);
    l.get_port("a").tieoff(0);
    i.get_port("a").tieoff(0);

    let manifest = top.manifest();
    let abs = |s: &str| std::path::absolute(s).unwrap();
    assert_eq!(
        manifest.sources,
        vec![abs(p), abs(c), abs(m), cwd.join(&relative)]
    );
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

    // Without the inline module, emitted RTL and manifest elaborate together.
    let top = ModDef::new("top2");
    let a = top.instantiate(&with_flit, Some("a"), None);
    let r = top.instantiate(&raw, Some("r"), None);
    let l = top.instantiate(&rel, Some("l"), None);
    a.get_port("in_i").tieoff(0);
    a.get_port("out_o").export_as("o");
    r.get_port("x").tieoff(0);
    r.get_port("f").tieoff(0);
    l.get_port("a").tieoff(0);
    let manifest = top.manifest();
    // `raw` is stubbed: its sources would define it twice; `child` brings
    // the package.
    assert_eq!(manifest.sources, vec![abs(p), abs(c), cwd.join(&relative)]);
    let files: Vec<&Path> = manifest.sources.iter().map(PathBuf::as_path).collect();
    elaborate(&files, &top.emit(EmitOptions::default()), "top2").unwrap_or_else(|e| panic!("{e}"));

    // Untyped, unimported designs have an empty manifest.
    assert_eq!(ModDef::new("empty").manifest(), Manifest::default());
}
