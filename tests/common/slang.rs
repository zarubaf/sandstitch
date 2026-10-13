// SPDX-License-Identifier: Apache-2.0

//! Running Slang on emitted RTL: strict elaboration together with the
//! design's sources, the nets Slang elaborates in a top module, and a
//! one-hot checker for the bits a selection reads.

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
    elaborate_with(files, &[], emitted, top)
}

/// [`elaborate`] with the sources, include directories, macro definitions
/// and library search of `manifest`: the build the manifest describes.
pub fn elaborate_manifest(manifest: &Manifest, emitted: &str, top: &str) -> Result<(), String> {
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
    elaborate_with(&files, &args, emitted, top)
}

fn elaborate_with(
    files: &[&Path],
    args: &[String],
    emitted: &str,
    top: &str,
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
    let clean = response
        .diagnostics
        .iter()
        .all(|d| d.severity < Severity::Warning);
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
