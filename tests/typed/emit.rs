// SPDX-License-Identifier: Apache-2.0

//! Typed emission: nets and ports declared with their driver's type, member
//! selects and indices for typed connections, the flat fallback, and the
//! build manifest. Every emitted design is elaborated with Slang together
//! with its sources, with width warnings enabled and no warning allowed;
//! typed selections are also checked bit by bit against hand-computed
//! layouts.

use std::path::{Path, PathBuf};

use num_bigint::BigInt;
use topstitch::*;

use crate::common::slang::{Sources, elaborate, elaborate_manifest};

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
    // A module in a file of its own, emitted as a stub below.
    let blk_sv =
        Sources::new(&["module blk (input logic [44:0] x, input cfg_pkg::flat_t f);\nendmodule\n"]);
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
    let b = blk_sv.paths()[0];
    let src = import(&[p, m], "src");
    let blk = import(&[p, b], "blk");
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
    // `blk` is emitted from the model (a stub), so it lists no sources.
    blk.set_usage(Usage::EmitStubAndStop);

    let mid = ModDef::new("mid");
    let c0 = mid.instantiate(&with_hdr, Some("c"), None);
    c0.get_port("in_i").export_as("in_i");
    c0.get_port("out_o").export_as("out_o");
    let top = ModDef::new("top");
    let a = top.instantiate(&with_flit, Some("a"), None);
    let s = top.instantiate(&src, Some("s"), None);
    let m0 = top.instantiate(&mid, Some("m0"), None);
    let m1 = top.instantiate(&mid, Some("m1"), None);
    let l = top.instantiate(&rel, Some("l"), None);
    let i = top.instantiate(&inline, Some("i"), None);
    let k = top.instantiate(&blk, Some("k"), None);
    a.get_port("in_i").tieoff(0);
    a.get_port("out_o").unused();
    for port in ["o", "arr", "off"] {
        s.get_port(port).unused();
    }
    m0.get_port("out_o").connect(&m1.get_port("in_i").index(0));
    m0.get_port("in_i").tieoff(0);
    m1.get_port("in_i").index(1).tieoff(0);
    m1.get_port("out_o").unused();
    l.get_port("a").tieoff(0);
    i.get_port("a").tieoff(0);
    k.get_port("x").tieoff(0);
    k.get_port("f").tieoff(0);

    let manifest = top.manifest();
    let abs = |s: &str| std::path::absolute(s).unwrap();
    assert_eq!(
        manifest.sources,
        vec![abs(p), abs(c), abs(m), cwd.join(&relative)]
    );
    assert!(!manifest.sources.contains(&abs(b)));
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
    let k = top.instantiate(&blk, Some("k"), None);
    let l = top.instantiate(&rel, Some("l"), None);
    a.get_port("in_i").tieoff(0);
    a.get_port("out_o").export_as("o");
    k.get_port("x").tieoff(0);
    k.get_port("f").tieoff(0);
    l.get_port("a").tieoff(0);
    let manifest = top.manifest();
    // `blk` is stubbed: its source would define it twice; `child` brings
    // the package.
    assert_eq!(manifest.sources, vec![abs(p), abs(c), cwd.join(&relative)]);
    assert!(manifest.incdirs.is_empty());
    assert!(manifest.defines.is_empty());
    elaborate_manifest(&manifest, &top.emit(EmitOptions::default()), "top2")
        .unwrap_or_else(|e| panic!("{e}"));

    // Untyped, unimported designs have an empty manifest.
    assert_eq!(ModDef::new("empty").manifest(), Manifest::default());
}

#[test]
fn manifest_reports_a_module_defined_twice() {
    // `src` and `raw` share a file; `raw` is stubbed, so the emitted RTL
    // defines it, while the file is listed for `src`.
    let sources = Sources::new(&[PKG, MODS]);
    let (src, raw) = (sources.import("src"), sources.import("raw"));
    raw.set_usage(Usage::EmitStubAndStop);
    let top = ModDef::new("top");
    let s = top.instantiate(&src, Some("s"), None);
    let r = top.instantiate(&raw, Some("r"), None);
    s.get_port("o").connect(&r.get_port("x"));
    for port in ["arr", "off"] {
        s.get_port(port).unused();
    }
    r.get_port("f").tieoff(0);

    let mods = sources.files[1].path().to_str().unwrap().to_string();
    let err = top.try_manifest().unwrap_err();
    assert_eq!(err.0.len(), 1, "{}", err.render());
    let d = &err.0[0];
    assert_eq!(d.code, diagnostic::codes::S0301);
    assert_eq!(
        d.message,
        format!("module `raw` would be defined twice: by the emitted RTL and by `{mods}`")
    );
    assert_eq!(d.primary.span, raw.get_port("x").source_span());
    let Span::Sv { file, .. } = &d.primary.span else {
        panic!("expected an SV span: {:?}", d.primary.span);
    };
    assert_eq!(file, &mods);
    // The failure is real: the emitted RTL and the sources clash.
    let emitted = top.emit(EmitOptions::default());
    let err = sources.elaborate(&emitted, "top").unwrap_err();
    assert!(err.contains("raw"), "{err}");
    // `manifest` panics with the rendered diagnostic.
    let rendered = top.try_manifest().unwrap_err().render();
    let payload =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| top.manifest())).unwrap_err();
    assert_eq!(payload.downcast_ref::<String>(), Some(&rendered));
    assert!(
        rendered.starts_with("error[S0301]: module `raw`"),
        "{rendered}"
    );
}

#[test]
fn manifest_folds_dot_segments() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let srcs = dir.path().join("srcs");
    std::fs::create_dir(&srcs).unwrap();
    std::fs::write(srcs.join("pkg.sv"), PKG).unwrap();
    std::fs::write(srcs.join("mods.sv"), MODS).unwrap();
    let cwd = std::env::current_dir().unwrap();
    let rel = srcs
        .strip_prefix(&cwd)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let a = [format!("{rel}/pkg.sv"), format!("{rel}/mods.sv")];
    let b = [format!("{rel}/../srcs/pkg.sv"), format!("./{rel}/mods.sv")];
    let import = |paths: &[String], name: &str| {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    };
    let (src, raw) = (import(&a, "src"), import(&b, "raw"));
    let top = ModDef::new("top");
    top.instantiate(&src, Some("s"), None);
    top.instantiate(&raw, Some("r"), None);
    assert_eq!(
        top.manifest().sources,
        vec![srcs.join("pkg.sv"), srcs.join("mods.sv")]
    );
}

#[test]
fn manifest_lists_incdirs_and_defines() {
    // `hdr_mod` needs its include directory and `WIDTH` to elaborate.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let inc = dir.path().join("inc");
    std::fs::create_dir(&inc).unwrap();
    std::fs::write(inc.join("extra.svh"), "`define EXTRA 2\n").unwrap();
    let hdr = dir.path().join("hdr_mod.sv");
    std::fs::write(
        &hdr,
        "`include \"extra.svh\"\nmodule hdr_mod (input logic [`WIDTH+`EXTRA-1:0] a);\nendmodule\n",
    )
    .unwrap();
    let other = dir.path().join("other.sv");
    std::fs::write(
        &other,
        "module other (input logic [`WIDTH-1:0] b);\nendmodule\n",
    )
    .unwrap();
    let cwd = std::env::current_dir().unwrap();
    // Relative at import, absolute in the manifest.
    let rel_inc = inc
        .strip_prefix(&cwd)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let import = |path: &Path, name: &str, incdirs: &[&str], width: &str| {
        let sources = [path.to_str().unwrap()];
        let defines = [("WIDTH", width)];
        ModDef::from_verilog_with_config(
            name,
            &ParserConfig {
                sources: &sources,
                incdirs,
                defines: &defines,
                ..Default::default()
            },
        )
    };
    let a = import(&hdr, "hdr_mod", &[rel_inc.as_str()], "4");
    assert_eq!(a.get_port("a").io().width(), 6);
    let b = import(&other, "other", &[], "4");

    let top = ModDef::new("top");
    top.instantiate(&a, Some("a"), None).get_port("a").tieoff(0);
    top.instantiate(&b, Some("b"), None).get_port("b").tieoff(0);
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![hdr.clone(), other.clone()]);
    assert_eq!(manifest.incdirs, vec![inc.clone()]);
    // Listed once although both imports define it.
    assert_eq!(
        manifest.defines,
        vec![("WIDTH".to_string(), "4".to_string())]
    );
    let emitted = top.emit(EmitOptions::default());
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
    // Without them the build fails: the manifest is what makes it work.
    let files: Vec<&Path> = manifest.sources.iter().map(PathBuf::as_path).collect();
    assert!(elaborate(&files, &emitted, "top").is_err());

    // Defined differently by two imports: S0302.
    let wide = import(&other, "other", &[], "8");
    let top = ModDef::new("top");
    top.instantiate(&a, Some("a"), None).get_port("a").tieoff(0);
    top.instantiate(&wide, Some("w"), None)
        .get_port("b")
        .tieoff(0);
    let err = top.try_manifest().unwrap_err();
    assert_eq!(err.0.len(), 1, "{}", err.render());
    assert_eq!(err.0[0].code, diagnostic::codes::S0302);
    assert_eq!(
        err.0[0].message,
        "macro `WIDTH` is defined as `4` for `hdr_mod` and as `8` for `other`"
    );
}

#[test]
fn manifest_lists_library_search() {
    // `uses_leaf` instantiates `leaf`, found by library search only.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let lib = dir.path().join("lib");
    std::fs::create_dir(&lib).unwrap();
    std::fs::write(
        lib.join("leaf.sv"),
        "module leaf (input logic a);\nendmodule\n",
    )
    .unwrap();
    let libfile = dir.path().join("cells.v");
    std::fs::write(&libfile, "module lcell (input logic c);\nendmodule\n").unwrap();
    let src = dir.path().join("uses_leaf.sv");
    std::fs::write(
        &src,
        "module uses_leaf (input logic x);\n  leaf u_leaf (.a(x));\n  lcell u_cell (.c(x));\nendmodule\n",
    )
    .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let rel = |p: &Path| p.strip_prefix(&cwd).unwrap().to_str().unwrap().to_string();
    let (rel_lib, rel_libfile) = (rel(&lib), rel(&libfile));
    let sources = [src.to_str().unwrap()];
    let m = ModDef::from_verilog_with_config(
        "uses_leaf",
        &ParserConfig {
            sources: &sources,
            libdirs: &[rel_lib.as_str()],
            libexts: &[".sv"],
            libfiles: &[rel_libfile.as_str()],
            ignore_unknown_modules: false,
            ..Default::default()
        },
    );
    // A second import with the same library search, spelled absolute.
    let src2 = dir.path().join("uses_leaf2.sv");
    std::fs::write(
        &src2,
        "module uses_leaf2 (input logic y);\n  leaf u_leaf (.a(y));\nendmodule\n",
    )
    .unwrap();
    let sources2 = [src2.to_str().unwrap()];
    let m2 = ModDef::from_verilog_with_config(
        "uses_leaf2",
        &ParserConfig {
            sources: &sources2,
            libdirs: &[lib.to_str().unwrap()],
            libexts: &[".sv"],
            libfiles: &[libfile.to_str().unwrap()],
            ignore_unknown_modules: false,
            ..Default::default()
        },
    );
    let top = ModDef::new("top");
    top.instantiate(&m, Some("u"), None).get_port("x").tieoff(0);
    top.instantiate(&m2, Some("u2"), None)
        .get_port("y")
        .tieoff(0);
    let manifest = top.manifest();
    // Listed once each.
    assert_eq!(manifest.sources, vec![src.clone(), src2.clone()]);
    assert_eq!(manifest.libdirs, vec![lib.clone()]);
    assert_eq!(manifest.libfiles, vec![libfile.clone()]);
    assert_eq!(manifest.libexts, vec![".sv".to_string()]);
    let emitted = top.emit(EmitOptions::default());
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
    let files: Vec<&Path> = manifest.sources.iter().map(PathBuf::as_path).collect();
    assert!(elaborate(&files, &emitted, "top").is_err());
}

/// A package whose struct names a type of another package.
const BASE_PKG: &str = "package base_pkg;\n  typedef logic [3:0] nib_t;\nendpackage\n";
const CFG_PKG: &str =
    "package cfg_pkg;\n  typedef struct packed { base_pkg::nib_t a; logic b; } t;\nendpackage\n";

/// Writes `text` to `name` in `dir`; returns its path.
fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

/// Imports `name` from `paths`.
fn import_files(paths: &[&Path], name: &str) -> ModDef {
    let paths: Vec<&str> = paths.iter().map(|p| p.to_str().unwrap()).collect();
    ModDef::from_verilog_with_config(
        name,
        &ParserConfig {
            sources: &paths,
            ..Default::default()
        },
    )
}

/// A top instantiating `m` as `u`, its input `i` tied off and its output
/// `o` exported.
fn top_of(m: &ModDef) -> ModDef {
    let top = ModDef::new("top");
    let u = top.instantiate(m, Some("u"), None);
    u.get_port("i").tieoff(0);
    u.get_port("o").export_as("o");
    top
}

/// The single S0303 of `top`'s manifest; panics unless that is all.
fn s0303(top: &ModDef) -> Diagnostic {
    let err = top.try_manifest().unwrap_err();
    assert_eq!(err.0.len(), 1, "{}", err.render());
    assert_eq!(err.0[0].code, diagnostic::codes::S0303, "{}", err.render());
    err.0[0].clone()
}

#[test]
fn stub_lists_package_and_its_dependencies() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let m = import_files(&[&base, &cfg, &m_sv], "m");
    m.set_usage(Usage::EmitStubAndStop);
    let top = top_of(&m);
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![base, cfg]);
    let emitted = top.emit(EmitOptions::default());
    assert!(emitted.contains("cfg_pkg::t i"), "{emitted}");
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
    // Without the packages the stub does not build.
    assert!(elaborate(&[], &emitted, "top").is_err());
}

#[test]
fn stubbed_specialization_lists_packages() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m #(parameter type T = logic) (input T i, output T o);\nendmodule\n",
    );
    let paths: Vec<&str> = [&base, &cfg, &m_sv]
        .iter()
        .map(|p| p.to_str().unwrap())
        .collect();
    let config = ParserConfig {
        sources: &paths,
        ..Default::default()
    };
    let overrides = [("T", ty("cfg_pkg::t"))];
    let generic = ModDef::from_verilog_with_config("m", &config);
    let specializations = [
        generic.try_parameterize(&overrides).unwrap(),
        ModDef::try_from_verilog_parameterized("m", &config, &overrides).unwrap(),
    ];
    for m in specializations {
        assert_eq!(m.get_port("i").io().width(), 5);
        m.set_usage(Usage::EmitStubAndStop);
        let top = top_of(&m);
        let manifest = top.manifest();
        assert_eq!(manifest.sources, vec![base.clone(), cfg.clone()]);
        assert_eq!(manifest.specializations.len(), 1);
        let emitted = top.emit(EmitOptions::default());
        assert!(emitted.contains("cfg_pkg::t i"), "{emitted}");
        // The stub is elaborated on its own: its parent passes the
        // specialization's overrides, which the stub does not declare (#59).
        let stub = m.get_name();
        elaborate_manifest(&manifest, &emitted, &stub).unwrap_or_else(|e| panic!("{e}"));
        assert!(elaborate(&[], &emitted, &stub).is_err());
    }
}

#[test]
fn two_stubs_share_a_package() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m1_sv = write(
        dir.path(),
        "m1.sv",
        "module m1 (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let m2_sv = write(
        dir.path(),
        "m2.sv",
        "module m2 (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let m1 = import_files(&[&base, &cfg, &m1_sv], "m1");
    let m2 = import_files(&[&base, &cfg, &m2_sv], "m2");
    m1.set_usage(Usage::EmitStubAndStop);
    m2.set_usage(Usage::EmitStubAndStop);
    let top = ModDef::new("top");
    let u1 = top.instantiate(&m1, Some("u1"), None);
    let u2 = top.instantiate(&m2, Some("u2"), None);
    u1.get_port("i").tieoff(0);
    u1.get_port("o").connect(&u2.get_port("i"));
    u2.get_port("o").export_as("o");
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![base, cfg]);
    let emitted = top.emit(EmitOptions::default());
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
}

/// A module whose struct port's package is declared in its own file.
const OWN_PKG: &str = "package own_pkg;\n  typedef struct packed { logic a; logic [2:0] b; } t;\nendpackage\n\
module r (input own_pkg::t i, output logic [3:0] o);\nendmodule\n";

#[test]
fn flat_port_needs_no_package() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let r_sv = write(dir.path(), "r.sv", OWN_PKG);
    // Re-emitted, with its struct input connected to a vector: the input is
    // emitted flat and names no package.
    let r = import_files(&[&r_sv], "r");
    r.set_usage(Usage::EmitDefinitionAndDescend);
    r.get_port("o").connect(&r.get_port("i"));
    let top = top_of(&r);
    let manifest = top.manifest();
    assert_eq!(manifest, Manifest::default());
    let emitted = top.emit(EmitOptions::default());
    assert!(!emitted.contains("own_pkg"), "{emitted}");
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));

    // Re-emitted with its input unused, the input keeps its type and needs
    // the package.
    let r = import_files(&[&r_sv], "r");
    r.set_usage(Usage::EmitDefinitionAndDescend);
    r.get_port("i").unused();
    r.get_port("o").tieoff(0);
    let top = top_of(&r);
    let emitted = top.emit(EmitOptions::default());
    assert!(emitted.contains("own_pkg::t i"), "{emitted}");
    s0303(&top);

    // Stubbed, the same module declares the input with its type and needs
    // the package.
    let r = import_files(&[&r_sv], "r");
    r.set_usage(Usage::EmitStubAndStop);
    let top = top_of(&r);
    let emitted = top.emit(EmitOptions::default());
    assert!(emitted.contains("own_pkg::t i"), "{emitted}");
    s0303(&top);
}

#[test]
fn package_in_module_file_is_s0303() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let r_sv = write(dir.path(), "r.sv", OWN_PKG);
    let r = import_files(&[&r_sv], "r");
    r.set_usage(Usage::EmitStubAndStop);
    let d = s0303(&top_of(&r));
    assert_eq!(
        d.message,
        format!(
            "package `own_pkg` needed by emitted module `r` is declared in the module's own \
             file `{}`",
            r_sv.display()
        )
    );
    assert_eq!(d.primary.span, r.get_port("i").source_span());
    assert_eq!(
        d.helps,
        vec!["declare the package in a file of its own and list it among the import's sources"]
    );
}

#[test]
fn package_outside_sources_is_s0303() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    // `cfg_pkg.sv` reaches Slang as an extra argument, not as a source.
    let sources = [base.to_str().unwrap(), m_sv.to_str().unwrap()];
    let m = ModDef::from_verilog_with_config(
        "m",
        &ParserConfig {
            sources: &sources,
            extra_arguments: &[cfg.to_str().unwrap()],
            ..Default::default()
        },
    );
    m.set_usage(Usage::EmitStubAndStop);
    let d = s0303(&top_of(&m));
    assert_eq!(
        d.message,
        format!(
            "package `cfg_pkg` needed by emitted module `m` is declared in `{}`, which is not \
             a source or library of its import",
            cfg.display()
        )
    );
    assert_eq!(d.primary.span, m.get_port("i").source_span());

    // As a library file, the package is listed with the library.
    let m = ModDef::from_verilog_with_config(
        "m",
        &ParserConfig {
            sources: &sources,
            libfiles: &[cfg.to_str().unwrap()],
            ..Default::default()
        },
    );
    m.set_usage(Usage::EmitStubAndStop);
    let top = top_of(&m);
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![base]);
    assert_eq!(manifest.libfiles, vec![cfg]);
    let emitted = top.emit(EmitOptions::default());
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn defining_file_in_header_is_s0303() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let header = write(
        dir.path(),
        "m_ports.svh",
        "input cfg_pkg::t i, output cfg_pkg::t o\n",
    );
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m (\n`include \"m_ports.svh\"\n);\nendmodule\n",
    );
    let sources = [
        base.to_str().unwrap(),
        cfg.to_str().unwrap(),
        m_sv.to_str().unwrap(),
    ];
    let m = ModDef::from_verilog_with_config(
        "m",
        &ParserConfig {
            sources: &sources,
            incdirs: &[dir.path().to_str().unwrap()],
            ..Default::default()
        },
    );
    m.set_usage(Usage::EmitStubAndStop);
    let d = s0303(&top_of(&m));
    assert_eq!(
        d.message,
        "the file defining emitted module `m` is not a source of its import, so the packages \
         it needs cannot be listed"
    );
    let Span::Sv { file, .. } = &d.primary.span else {
        panic!("expected an SV span: {:?}", d.primary.span);
    };
    assert_eq!(Path::new(file), header);
}

#[test]
fn package_of_a_listed_source_in_defining_file_is_s0303() {
    // `cfg_pkg.sv` would be listed for the stub, but it needs `base_pkg`,
    // which only the stub's own file declares.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let m_sv = write(
        dir.path(),
        "m.sv",
        &format!("{BASE_PKG}module m (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n"),
    );
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m = import_files(&[&m_sv, &cfg], "m");
    m.set_usage(Usage::EmitStubAndStop);
    let d = s0303(&top_of(&m));
    assert_eq!(
        d.message,
        format!(
            "package `base_pkg` of the import listed for `cfg_pkg` is declared in `{}`, which \
             defines emitted module `m`",
            m_sv.display()
        )
    );
    assert_eq!(d.primary.span, m.get_port("i").source_span());
}

#[test]
fn enum_cast_lists_its_package() {
    // The stub declares its enum input flat; only the parent's cast names
    // the package.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let pkg = write(
        dir.path(),
        "pkg.sv",
        "package pkg;\n  typedef enum logic [1:0] { A, B, C } e_t;\nendpackage\n",
    );
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m (input pkg::e_t s);\nendmodule\n",
    );
    let m = import_files(&[&pkg, &m_sv], "m");
    m.set_usage(Usage::EmitStubAndStop);
    let top = ModDef::new("top");
    top.instantiate(&m, Some("u"), None)
        .get_port("s")
        .export_as("s");
    let emitted = top.emit(EmitOptions::default());
    assert!(emitted.contains("pkg::e_t'("), "{emitted}");
    assert!(!emitted.contains("pkg::e_t s"), "{emitted}");
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![pkg]);
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
    assert!(elaborate(&[], &emitted, "top").is_err());
}

#[test]
fn package_outside_sources_of_a_descended_import_is_s0303() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    let m_sv = write(
        dir.path(),
        "m.sv",
        "module m (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let sources = [base.to_str().unwrap(), m_sv.to_str().unwrap()];
    let m = ModDef::from_verilog_with_config(
        "m",
        &ParserConfig {
            sources: &sources,
            extra_arguments: &[cfg.to_str().unwrap()],
            ..Default::default()
        },
    );
    // `m` is descended into; the top's exported port is typed.
    let top = ModDef::new("top");
    let u = top.instantiate(&m, Some("u"), None);
    u.get_port("i").export_as("i");
    u.get_port("o").export_as("o");
    let d = s0303(&top);
    assert_eq!(
        d.message,
        format!(
            "package `cfg_pkg` used by the emitted RTL is declared in `{}`, which is not a \
             source or library of the import of `m`",
            cfg.display()
        )
    );
}

#[test]
fn parent_port_typed_by_a_child_lists_its_package() {
    // The child is re-emitted with its input used flat, so it names no
    // package; the parent exports that input, typed.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let pkg = write(
        dir.path(),
        "own_pkg.sv",
        "package own_pkg;\n  typedef struct packed { logic a; logic [2:0] b; } t;\nendpackage\n",
    );
    let r_sv = write(
        dir.path(),
        "r.sv",
        "module r (input own_pkg::t i, output logic [3:0] o);\nendmodule\n",
    );
    let r = import_files(&[&pkg, &r_sv], "r");
    r.set_usage(Usage::EmitDefinitionAndDescend);
    r.get_port("o").connect(&r.get_port("i"));
    let top = ModDef::new("top");
    let u = top.instantiate(&r, Some("u"), None);
    u.get_port("i").export_as("i");
    u.get_port("o").export_as("o");
    let emitted = top.emit(EmitOptions::default());
    assert!(emitted.contains("own_pkg::t i"), "{emitted}");
    let manifest = top.manifest();
    assert_eq!(manifest.sources, vec![pkg]);
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn two_stubs_from_one_import_list_its_packages_once() {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let base = write(dir.path(), "base_pkg.sv", BASE_PKG);
    let cfg = write(dir.path(), "cfg_pkg.sv", CFG_PKG);
    // One import of four files; each stub's file is left out for both.
    let m1_sv = write(
        dir.path(),
        "m1.sv",
        "module m1 (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let m2_sv = write(
        dir.path(),
        "m2.sv",
        "module m2 (input cfg_pkg::t i, output cfg_pkg::t o);\nendmodule\n",
    );
    let files: [&Path; 4] = [&base, &cfg, &m1_sv, &m2_sv];
    let m1 = import_files(&files, "m1");
    let m2 = import_files(&files, "m2");
    m1.set_usage(Usage::EmitStubAndStop);
    m2.set_usage(Usage::EmitStubAndStop);
    let top = ModDef::new("top");
    let u1 = top.instantiate(&m1, Some("u1"), None);
    let u2 = top.instantiate(&m2, Some("u2"), None);
    u1.get_port("i").tieoff(0);
    u1.get_port("o").connect(&u2.get_port("i"));
    u2.get_port("o").export_as("o");
    let manifest = top
        .try_manifest()
        .unwrap_or_else(|e| panic!("{}", e.render()));
    assert_eq!(manifest.sources, vec![base, cfg]);
    let emitted = top.emit(EmitOptions::default());
    elaborate_manifest(&manifest, &emitted, "top").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn package_no_import_declares_is_s0303() {
    // The package is declared only in text passed to `from_verilog`, which
    // the manifest cannot list; the generated top exports a port of it.
    let inline = ModDef::from_verilog(
        "txt",
        "package txt_pkg;\n  typedef struct packed { logic a; logic b; } t;\nendpackage\n\
         module txt (input txt_pkg::t i);\nendmodule\n",
        false,
        false,
    );
    let top = ModDef::new("top");
    top.instantiate(&inline, Some("u"), None)
        .get_port("i")
        .export_as("i");
    assert!(top.emit(EmitOptions::default()).contains("txt_pkg::t i"));
    let d = s0303(&top);
    assert_eq!(
        d.message,
        "package `txt_pkg` used by the emitted RTL is declared by no import of the design"
    );
}
