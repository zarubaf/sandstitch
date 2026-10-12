// SPDX-License-Identifier: Apache-2.0

//! Running Slang on emitted RTL: strict elaboration together with the
//! design's sources, and a one-hot checker for the bits a selection reads.

use std::path::Path;
use std::process::Command;

use slang_rs::str2tmpfile;
use tempfile::NamedTempFile;
use topstitch::*;

/// The Slang binary: `SLANG_PATH`, or `slang` on the `PATH`.
pub fn slang() -> String {
    std::env::var("SLANG_PATH").unwrap_or_else(|_| "slang".into())
}

/// Elaborates `files` plus `emitted` under `top` with width and multiple
/// driver warnings on; returns Slang's output unless the build has no errors
/// and no warnings. (`-Wpacked-array-conv` stays off: flat nets and
/// concatenations connect to packed-array ports by implicit conversion by
/// design.)
pub fn elaborate(files: &[&Path], emitted: &str, top: &str) -> Result<(), String> {
    let rtl = str2tmpfile(emitted).unwrap();
    let output = Command::new(slang())
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
