// SPDX-License-Identifier: Apache-2.0

//! Running Slang on emitted RTL: strict elaboration together with the
//! design's sources, the nets Slang elaborates in a top module, a one-hot
//! checker for the bits a selection reads, and a checker that evaluates a
//! hand-written connection map against the emitted module.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use slang_rs::str2tmpfile;
use slang_rs::sys::{self, Request, Severity, Stage};
use tempfile::NamedTempFile;
use topstitch::*;

/// Runs Slang in-process with `args` and `files`; panics if Slang itself
/// fails (an exception, not a diagnostic).
fn run_slang(files: &[&Path], args: Vec<String>, design: sys::DesignQuery) -> sys::Response {
    let mut all = args;
    all.extend(files.iter().map(|f| f.to_str().unwrap().to_string()));
    sys::run(&Request {
        args: all,
        stage: Stage::Elaborate,
        design,
    })
    .unwrap_or_else(|e| panic!("running Slang: {e}"))
}

/// Elaborates `files` plus `emitted` under `top` with width and multiple
/// driver warnings on; returns Slang's output unless the build has no errors
/// and no warnings. (`-Wpacked-array-conv` stays off: flat nets and
/// concatenations connect to packed-array ports by implicit conversion by
/// design.)
pub fn elaborate(files: &[&Path], emitted: &str, top: &str) -> Result<(), String> {
    elaborate_with(files, &[], emitted, top, None)
}

/// [`elaborate`] with the sources, include directories, macro definitions
/// and library search of `manifest`: the build the manifest describes.
pub fn elaborate_manifest(manifest: &Manifest, emitted: &str, top: &str) -> Result<(), String> {
    elaborate_manifest_with(manifest, emitted, top, None)
}

/// [`elaborate_manifest`], tolerating only the implicit width conversion
/// warnings ([`VENDORED_WIDTH_CODES`]) of files under `vendored`:
/// third-party RTL that is not edited, whose conversions are its own.
/// Errors, other warnings, and any warning elsewhere still fail. Used by
/// the FlooNoC design tests, which include this file.
#[allow(dead_code)]
pub fn elaborate_manifest_vendored(
    manifest: &Manifest,
    emitted: &str,
    top: &str,
    vendored: &Path,
) -> Result<(), String> {
    elaborate_manifest_with(manifest, emitted, top, Some(vendored))
}

/// The Slang codes of `-Wwidth-trunc` and `-Wwidth-expand`, which
/// FlooNoC's `floo_nw_chimney` and `floo_meta_buffer` raise with every
/// configuration.
pub const VENDORED_WIDTH_CODES: [&str; 2] = ["WidthTruncate", "WidthExpand"];

fn elaborate_manifest_with(
    manifest: &Manifest,
    emitted: &str,
    top: &str,
    vendored: Option<&Path>,
) -> Result<(), String> {
    let files: Vec<&Path> = manifest.sources.iter().map(PathBuf::as_path).collect();
    let mut args = Vec::new();
    for incdir in &manifest.incdirs {
        args.push("-I".to_string());
        args.push(incdir.to_str().unwrap().to_string());
    }
    for (name, value) in &manifest.defines {
        args.push("-D".to_string());
        args.push(format!("{name}={value}"));
    }
    for (flag, paths) in [("-v", &manifest.libfiles), ("-y", &manifest.libdirs)] {
        for path in paths {
            args.push(flag.to_string());
            args.push(path.to_str().unwrap().to_string());
        }
    }
    for ext in &manifest.libexts {
        args.push("-Y".to_string());
        args.push(ext.clone());
    }
    elaborate_with(&files, &args, emitted, top, vendored)
}

fn elaborate_with(
    files: &[&Path],
    args: &[String],
    emitted: &str,
    top: &str,
    vendored: Option<&Path>,
) -> Result<(), String> {
    let rtl = str2tmpfile(emitted).unwrap();
    let mut all: Vec<String> = args.to_vec();
    for arg in [
        "--top",
        top,
        "-Wport-width-trunc",
        "-Wport-width-expand",
        "-Wwidth-trunc",
        "-Wwidth-expand",
        "-Wmultiple-cont-assigns",
    ] {
        all.push(arg.to_string());
    }
    let mut sources: Vec<&Path> = files.to_vec();
    sources.push(rtl.path());
    let response = run_slang(&sources, all, sys::DesignQuery::None);
    let clean = response.diagnostics.iter().all(|d| {
        d.severity < Severity::Warning
            || (d.severity == Severity::Warning
                && VENDORED_WIDTH_CODES.contains(&d.code.as_str())
                && vendored.is_some_and(|v| Path::new(&d.file).starts_with(v)))
    });
    if response.ok && clean {
        Ok(())
    } else {
        Err(response.rendered)
    }
}

/// Every net and variable Slang elaborates in the body of `top` (ports
/// included, by their internal symbol), as `name -> (type, width)`. The
/// type is Slang's display of it; the width is its bitstream width (`$bits`).
pub fn top_nets(
    files: &[&Path],
    emitted: &str,
    top: &str,
) -> Result<BTreeMap<String, (String, usize)>, String> {
    let rtl = str2tmpfile(emitted).unwrap();
    let mut sources: Vec<&Path> = files.to_vec();
    sources.push(rtl.path());
    let response = run_slang(
        &sources,
        vec!["--top".to_string(), top.to_string()],
        sys::DesignQuery::WithSignals,
    );
    if !response.ok {
        return Err(response.rendered);
    }
    let design = response
        .design
        .unwrap_or_else(|| panic!("Slang returned no design"));
    let signals = design
        .signals(top)
        .unwrap_or_else(|| panic!("no top-level instance `{top}` in Slang's design"));
    Ok(signals
        .iter()
        .map(|s| (s.name.clone(), (s.type_text.clone(), s.width)))
        .collect())
}

/// One instance port connection of an emitted top and, by hand, where each
/// of its bits comes from (input) or goes to (output).
pub struct Connection<'a> {
    pub inst: &'a str,
    pub port: &'a str,
    /// The port's direction: an output drives the connection expression.
    pub output: bool,
    /// The port's width.
    pub width: usize,
    /// `(port_msb, port_lsb, net, net_msb, net_lsb)`: the port bits are
    /// the net bits, in order.
    pub segments: &'a [(usize, usize, &'a str, usize, usize)],
}

/// The expression connected to `.port(...)` of instance `inst` in the
/// emitted text.
pub fn connection_expr(emitted: &str, inst: &str, port: &str) -> Option<String> {
    let mut lines = emitted.lines();
    lines.find(|l| l.ends_with(&format!(" {inst} (")))?;
    let open = format!(".{port}(");
    lines
        .take_while(|l| l.trim() != ");")
        .find_map(|l| l.trim().strip_prefix(&open))
        .map(|rest| {
            let rest = rest.strip_suffix(',').unwrap_or(rest);
            rest.strip_suffix(')').unwrap().to_string()
        })
}

/// Checks with Slang, at elaboration time, that each connection expression
/// of the emitted text moves bits as its segments say. The nets of the top
/// are declared as function locals of the types in `nets`; one bit at a
/// time is set (in a source net for an input, in the port value for an
/// output), the expression is evaluated or assigned, and the result must
/// be exactly the one-hot bit the segment names.
pub fn check_connections(
    files: &[&Path],
    emitted: &str,
    nets: &[(&str, (&str, usize))],
    connections: &[Connection],
) -> Result<(), String> {
    let width = |net: &str| {
        nets.iter()
            .find(|(n, _)| *n == net)
            .unwrap_or_else(|| panic!("no net `{net}`"))
            .1
            .1
    };
    let mut body = String::from("module connchk;\n");
    let mut locals = String::new();
    for (name, (ty, _)) in nets {
        body += &format!("  typedef {ty} t_{name};\n");
        locals += &format!("    t_{name} {name};\n");
    }
    for (name, _) in nets {
        locals += &format!("    {name} = '0;\n");
    }
    for (i, c) in connections.iter().enumerate() {
        let covered: usize = c.segments.iter().map(|s| s.0 - s.1 + 1).sum();
        assert_eq!(covered, c.width, "segments of {}.{}", c.inst, c.port);
        let expr = connection_expr(emitted, c.inst, c.port)
            .unwrap_or_else(|| panic!("no connection {}.{} in:\n{emitted}", c.inst, c.port));
        let w = c.width;
        for (s, &(port_msb, port_lsb, net, net_msb, net_lsb)) in c.segments.iter().enumerate() {
            let n = port_msb - port_lsb + 1;
            assert_eq!(
                n,
                net_msb - net_lsb + 1,
                "segment {s} of {}.{}",
                c.inst,
                c.port
            );
            let nw = width(net);
            let f = format!("c{i}_{s}");
            let (src, dst, rw) = if c.output {
                body += &format!(
                    "  function automatic logic [{}:0] {f}(int k);\n{locals}    \
                     {expr} = {w}'(1) << k;\n    return {net};\n  endfunction\n",
                    nw - 1
                );
                (port_lsb, net_lsb, nw)
            } else {
                body += &format!(
                    "  function automatic logic [{}:0] {f}(int k);\n{locals}    \
                     {net} = t_{net}'({nw}'(1) << k);\n    return {expr};\n  endfunction\n",
                    w - 1
                );
                (net_lsb, port_lsb, w)
            };
            body += &format!(
                "  for (genvar j = 0; j < {n}; j++) begin : g{i}_{s}\n    \
                 if ({f}({src} + j) !== {rw}'(1) << ({dst} + j))\n      \
                 $error(\"{}.{} segment {s}: bit %0d\", j);\n  end\n",
                c.inst, c.port
            );
        }
    }
    body += "endmodule\n";
    // Only Slang's output: the body names every check.
    elaborate(files, &body, "connchk")
}

/// Where bits of an end come from, by hand.
pub enum Driver<'a> {
    /// Bits `[msb:lsb]` of end `end`: `"inst.port"` or a port `"p"` of the module.
    End(&'a str, usize, usize),
    /// A constant; its width is the sink range's width.
    Const(u128),
    /// Intentionally unused: instance outputs, module inputs and unconnected
    /// instance inputs. No other end's expression may read these bits.
    Unused,
}

/// Bits `[msb:lsb]` of `end` come from `driver`.
pub struct Expect<'a> {
    pub end: &'a str,
    pub msb: usize,
    pub lsb: usize,
    pub driver: Driver<'a>,
}

/// The text of `module <name>` through its `endmodule` in `emitted`.
pub fn module_text<'e>(emitted: &'e str, name: &str) -> &'e str {
    let mut start = None;
    let mut offset = 0;
    for line in emitted.split_inclusive('\n') {
        let trimmed = line.trim_end();
        match start {
            None => {
                let is_header = trimmed.strip_prefix("module ").is_some_and(|rest| {
                    rest.strip_prefix(name).is_some_and(|after| {
                        !after.starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    })
                });
                if is_header {
                    start = Some(offset);
                }
            }
            Some(from) => {
                if trimmed == "endmodule" {
                    return &emitted[from..offset + trimmed.len()];
                }
            }
        }
        offset += line.len();
    }
    panic!("no module `{name}` in:\n{emitted}");
}

/// The ends of a module's text: every module port (the last identifier of
/// each line of the header) and every `inst.port` connection line.
fn module_ends(text: &str) -> Vec<String> {
    let mut ends = Vec::new();
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    if header.trim_end().ends_with('(') {
        for line in lines.by_ref() {
            if line.trim() == ");" {
                break;
            }
            let decl = line.trim().trim_end_matches(',');
            if let Some(port) = decl.split_whitespace().last() {
                ends.push(port.to_string());
            }
        }
    }
    let mut inst: Option<String> = None;
    for line in lines {
        let t = line.trim();
        if t == ");" {
            inst = None;
        } else if let Some(i) = &inst {
            if let Some(port) = t.strip_prefix('.').and_then(|r| r.split('(').next()) {
                ends.push(format!("{i}.{port}"));
            }
        } else if t.ends_with(" (") && !t.starts_with('.') {
            inst = t
                .strip_suffix(" (")
                .and_then(|h| h.split_whitespace().last())
                .map(str::to_string);
        }
    }
    ends
}

/// One sink range of a check: `[msb:lsb]` of the expression of an end of
/// width `width`.
#[derive(Clone)]
struct Sink {
    expr: String,
    width: usize,
    msb: usize,
    lsb: usize,
}

/// A `chk_` function over the locals of a module: all locals set to `fill`,
/// optionally bit `chk_k` of `write` (an expression of the given width) set,
/// `assigns` applied, then the concatenation of `sinks` returned. Returns the
/// text and the width of the result.
fn chk_function(
    name: &str,
    nets: &BTreeMap<String, (String, usize)>,
    assigns: &[(String, String)],
    write: Option<(&str, usize)>,
    fill: &str,
    sinks: &[Sink],
) -> (String, usize) {
    let total: usize = sinks.iter().map(|s| s.msb - s.lsb + 1).sum();
    let args = if write.is_some() { "int chk_k" } else { "" };
    let mut f = format!(
        "  function automatic logic [{}:0] {name}({args});\n",
        total - 1
    );
    for (i, s) in sinks.iter().enumerate() {
        f += &format!("    logic [{}:0] chk_v{i};\n", s.width - 1);
    }
    for net in nets.keys() {
        f += &format!("    chk_t_{net} {net};\n");
    }
    for net in nets.keys() {
        f += &format!("    {net} = {fill};\n");
    }
    if let Some((expr, w)) = write {
        f += &format!("    {expr} = {w}'(1) << chk_k;\n");
    }
    // As many rounds as there are assigns: any order settles.
    for _ in assigns {
        for (lhs, rhs) in assigns {
            f += &format!("    {lhs} = {rhs};\n");
        }
    }
    for (i, s) in sinks.iter().enumerate() {
        f += &format!("    chk_v{i} = {};\n", s.expr);
    }
    let parts: Vec<String> = sinks
        .iter()
        .enumerate()
        .map(|(i, s)| format!("chk_v{i}[{}:{}]", s.msb, s.lsb))
        .collect();
    f += &format!("    return {{{}}};\n  endfunction\n", parts.join(", "));
    (f, total)
}

/// Whether `expr` is a `<type>'(<expr>)` cast.
fn is_type_cast(expr: &str) -> bool {
    expr.find("'(").is_some_and(|p| {
        let ty = &expr[..p];
        !ty.is_empty()
            && !ty.starts_with(|c: char| c.is_ascii_digit())
            && ty
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
            && expr.ends_with(')')
    })
}

/// Checks `map` against module `module` of `emitted`. `ends` lists every
/// end of the module (each instance port as `"inst.port"`, each module
/// port) with its width, by hand. Err carries Slang's output, or a
/// message naming an end missing from `ends`.
///
/// Slang evaluates the emitted text: for each bit of a driver, a one-hot
/// value is written into the driver's end (all other nets zero), the
/// module's `assign`s are applied, and the sink range must read exactly
/// that bit. `connection_expr` and the assign reader parse the emitter's
/// layout only.
pub fn check_map(
    files: &[&Path],
    emitted: &str,
    module: &str,
    ends: &[(&str, usize)],
    map: &[Expect],
) -> Result<(), String> {
    let text = module_text(emitted, module);
    for end in module_ends(text) {
        if !ends.iter().any(|(e, _)| *e == end) {
            return Err(format!("end {end} missing from ends"));
        }
    }
    let width_of = |end: &str| -> Result<usize, String> {
        ends.iter()
            .find(|(e, _)| *e == end)
            .map(|(_, w)| *w)
            .ok_or_else(|| format!("end {end} in the map but not in ends"))
    };

    // Completeness of the map: each bit is at most one entry's end, and a
    // bit that is none lies in some driver range.
    let mut mapped: BTreeMap<&str, Vec<bool>> = BTreeMap::new();
    for (end, w) in ends {
        mapped.insert(end, vec![false; *w]);
    }
    for e in map {
        let w = width_of(e.end)?;
        if e.msb < e.lsb || e.msb >= w {
            return Err(format!("{} [{}:{}] outside the end", e.end, e.msb, e.lsb));
        }
        let bits = mapped.get_mut(e.end).unwrap();
        for (b, taken) in bits.iter_mut().enumerate().take(e.msb + 1).skip(e.lsb) {
            if *taken {
                return Err(format!("{} bit {b} mapped twice", e.end));
            }
            *taken = true;
        }
        if let Driver::End(src, smsb, slsb) = e.driver
            && (smsb < slsb || smsb >= width_of(src)? || smsb - slsb != e.msb - e.lsb)
        {
            return Err(format!(
                "{} [{}:{}] driver {src} [{smsb}:{slsb}] does not match",
                e.end, e.msb, e.lsb
            ));
        }
    }
    for (end, bits) in &mapped {
        for (b, is_sink) in bits.iter().enumerate() {
            let driven = map.iter().any(
                |e| matches!(e.driver, Driver::End(s, m, l) if s == *end && (l..=m).contains(&b)),
            );
            if !is_sink && !driven {
                return Err(format!("{end} bit {b} not covered by the map"));
            }
        }
    }

    let expr_of = |end: &str| -> Result<String, String> {
        let expr = match end.split_once('.') {
            Some((inst, port)) => connection_expr(text, inst, port)
                .ok_or_else(|| format!("no connection {end} in module {module}"))?,
            None => end.to_string(),
        };
        if is_type_cast(&expr) {
            return Err(format!("end {end}: type cast {expr} is unsupported"));
        }
        Ok(expr)
    };
    // The expression of an end that carries bits: empty means unconnected.
    let used_expr = |end: &str| -> Result<String, String> {
        let expr = expr_of(end)?;
        if expr.is_empty() {
            return Err(format!("end {end} is unconnected but mapped"));
        }
        Ok(expr)
    };
    let sink_of = |e: &Expect| -> Result<Sink, String> {
        Ok(Sink {
            expr: used_expr(e.end)?,
            width: width_of(e.end)?,
            msb: e.msb,
            lsb: e.lsb,
        })
    };
    let nets = top_nets(files, emitted, module)?;
    let assigns: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| {
            let a = l.trim().strip_prefix("assign ")?.strip_suffix(';')?;
            let (lhs, rhs) = a.split_once(" = ")?;
            Some((lhs.trim().to_string(), rhs.trim().to_string()))
        })
        .collect();

    let mut body = String::from("module chk_top;\n");
    for (net, (ty, _)) in &nets {
        body += &format!("  typedef {ty} chk_t_{net};\n");
    }
    for (i, e) in map.iter().enumerate() {
        let n = e.msb - e.lsb + 1;
        let label = format!("{} [{}:{}]", e.end, e.msb, e.lsb);
        match e.driver {
            Driver::End(src, _, slsb) => {
                let src_expr = used_expr(src)?;
                let sink = [sink_of(e)?];
                let (base, _) =
                    chk_function(&format!("chk_b{i}"), &nets, &assigns, None, "'0", &sink);
                let (f, _) = chk_function(
                    &format!("chk_f{i}"),
                    &nets,
                    &assigns,
                    Some((&src_expr, width_of(src)?)),
                    "'0",
                    &sink,
                );
                body += &base;
                body += &f;
                body += &format!(
                    "  localparam logic [{}:0] chk_base{i} = chk_b{i}();\n  \
                     for (genvar chk_j = 0; chk_j < {n}; chk_j++) begin : chk_g{i}\n    \
                     if ((chk_f{i}({slsb} + chk_j) ^ chk_base{i}) !== {n}'(1) << chk_j)\n      \
                     $error(\"{label} bit %0d\", chk_j);\n  end\n",
                    n - 1
                );
            }
            Driver::Const(v) => {
                let sink = [sink_of(e)?];
                let mask = if n >= 128 {
                    u128::MAX
                } else {
                    (1u128 << n) - 1
                };
                let k = format!("{n}'h{:x}", v & mask);
                let (zero, _) =
                    chk_function(&format!("chk_z{i}"), &nets, &assigns, None, "'0", &sink);
                let (ones, _) =
                    chk_function(&format!("chk_o{i}"), &nets, &assigns, None, "'1", &sink);
                body += &zero;
                body += &ones;
                body += &format!(
                    "  if (chk_z{i}() !== {k} || chk_o{i}() !== {k})\n    \
                     $error(\"{label} const\");\n"
                );
            }
            Driver::Unused => {
                let expr = expr_of(e.end)?;
                if expr.is_empty() {
                    continue;
                }
                // Every other end's whole expression must stay unchanged.
                let mut readers: Vec<Sink> = Vec::new();
                for (other, w) in ends {
                    if *other == e.end {
                        continue;
                    }
                    let oexpr = expr_of(other)?;
                    if !oexpr.is_empty() {
                        readers.push(Sink {
                            expr: oexpr,
                            width: *w,
                            msb: w - 1,
                            lsb: 0,
                        });
                    }
                }
                if readers.is_empty() {
                    continue;
                }
                let (base, total) =
                    chk_function(&format!("chk_ub{i}"), &nets, &assigns, None, "'0", &readers);
                body += &base;
                body += &format!(
                    "  localparam logic [{}:0] chk_ubase{i} = chk_ub{i}();\n",
                    total - 1
                );
                let (f, _) = chk_function(
                    &format!("chk_u{i}"),
                    &nets,
                    &assigns,
                    Some((&expr, width_of(e.end)?)),
                    "'0",
                    &readers,
                );
                body += &f;
                body += &format!(
                    "  for (genvar chk_j = 0; chk_j < {n}; chk_j++) begin : chk_g{i}\n    \
                     if (chk_u{i}({} + chk_j) !== chk_ubase{i})\n      \
                     $error(\"{label} unused bit %0d\", chk_j);\n  end\n",
                    e.lsb
                );
            }
        }
    }
    body += "endmodule\n";
    elaborate(files, &format!("{emitted}\n{body}"), "chk_top")
}

/// Sources written to temp files; kept alive while the design is in use.
pub struct Sources {
    pub files: Vec<NamedTempFile>,
}

impl Sources {
    pub fn new(sources: &[&str]) -> Self {
        Sources {
            files: sources.iter().map(|s| str2tmpfile(s).unwrap()).collect(),
        }
    }

    pub fn paths(&self) -> Vec<&str> {
        self.files
            .iter()
            .map(|f| f.path().to_str().unwrap())
            .collect()
    }

    pub fn import(&self, name: &str) -> ModDef {
        let paths = self.paths();
        let cfg = ParserConfig {
            sources: &paths,
            ..Default::default()
        };
        ModDef::from_verilog_with_config(name, &cfg)
    }

    pub fn elaborate(&self, emitted: &str, top: &str) -> Result<(), String> {
        let files: Vec<&Path> = self.files.iter().map(|f| f.path()).collect();
        elaborate(&files, emitted, top)
    }

    /// Emits `top` with `options`, checks that Slang elaborates it, and
    /// returns the text.
    pub fn emit_checked_with(&self, top: &ModDef, options: EmitOptions) -> String {
        let emitted = top.emit(options);
        if let Err(slang) = self.elaborate(&emitted, &top.get_name()) {
            panic!("Slang rejects the emitted RTL:\n{emitted}\n{slang}");
        }
        emitted
    }

    pub fn emit_checked(&self, top: &ModDef) -> String {
        self.emit_checked_with(top, EmitOptions::default())
    }

    /// Checks with Slang that each `(select, msb, lsb)` applied to a value
    /// of type `ty` reads exactly bits `[msb:lsb]` of it: a value with only
    /// bit `lsb` (resp. `msb`) set must read as the selection's lowest
    /// (resp. highest) bit, and the selection must be `msb - lsb + 1` bits.
    pub fn check_bits(&self, ty: &str, selects: &[(&str, usize, usize)]) -> Result<(), String> {
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

    pub fn assert_bits(&self, ty: &str, selects: &[(&str, usize, usize)]) {
        self.check_bits(ty, selects)
            .unwrap_or_else(|e| panic!("{e}"));
    }
}
