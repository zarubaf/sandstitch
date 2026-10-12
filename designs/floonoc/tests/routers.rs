// SPDX-License-Identifier: Apache-2.0

//! Two router configurations import as distinct specializations, and a 2x2
//! mesh of each emits RTL that Slang elaborates with the build manifest and
//! strict width warnings.

use std::io::Write as _;
use std::process::Command;

use floonoc::*;
use topstitch::{EmitOptions, Manifest};

/// Narrow 32-bit and wide 64-bit data, 1-bit coordinates, one shared wide
/// link.
fn small() -> FlooConfig {
    let mut cfg = FlooConfig::synth("small");
    cfg.narrow.addr_width = 32;
    cfg.narrow.data_width = 32;
    cfg.wide.addr_width = 32;
    cfg.wide.data_width = 64;
    cfg.x_bits = 1;
    cfg.y_bits = 1;
    cfg.wide_rw_decouple = WideRwDecouple::None;
    cfg
}

/// Exactly `floo_synth_nw_pkg`: narrow 64-bit and wide 512-bit data.
fn big() -> FlooConfig {
    FlooConfig::synth("big")
}

/// Elaborates the manifest's build plus `emitted` under `top`, with width
/// and multiple-driver warnings on; Slang's output unless it reports no
/// errors and no warnings.
fn elaborate_manifest(manifest: &Manifest, emitted: &str, top: &str) -> Result<(), String> {
    let mut rtl = tempfile::Builder::new().suffix(".sv").tempfile().unwrap();
    rtl.write_all(emitted.as_bytes()).unwrap();
    let mut cmd = Command::new(std::env::var("SLANG_PATH").unwrap_or_else(|_| "slang".into()));
    for incdir in &manifest.incdirs {
        cmd.arg("-I").arg(incdir);
    }
    for (name, value) in &manifest.defines {
        cmd.arg("-D").arg(format!("{name}={value}"));
    }
    for libfile in &manifest.libfiles {
        cmd.arg("-v").arg(libfile);
    }
    for libdir in &manifest.libdirs {
        cmd.arg("-y").arg(libdir);
    }
    for libext in &manifest.libexts {
        cmd.arg("-Y").arg(libext);
    }
    let output = cmd
        .args(&manifest.sources)
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

/// The expression connected to `.port(...)` of instance `inst` in `emitted`.
fn connection<'a>(emitted: &'a str, inst: &str, port: &str) -> &'a str {
    let body = &emitted[emitted
        .find(&format!(") {inst} ("))
        .unwrap_or_else(|| panic!("no instance {inst}"))..];
    let body = &body[..body.find(");").unwrap()];
    let start = body
        .find(&format!(".{port}("))
        .unwrap_or_else(|| panic!("no port {port} on {inst}"))
        + port.len()
        + 2;
    let line = &body[start..];
    &line[..line.find("),\n").or_else(|| line.find(")\n")).unwrap()]
}

fn width(router: &topstitch::ModDef, port: &str) -> usize {
    router
        .get_port(port)
        .port_type()
        .unwrap_or_else(|| panic!("{port} is untyped"))
        .width()
}

#[test]
fn configurations_are_distinct_specializations() {
    let small_router = router(&small()).unwrap();
    let big_router = router(&big()).unwrap();
    let small_key = small_router.specialization_key().unwrap();
    let big_key = big_router.specialization_key().unwrap();
    assert_ne!(small_key, big_key);
    assert_eq!(
        router(&small()).unwrap().specialization_key().unwrap(),
        small_key,
        "an equal configuration is the same specialization"
    );
    let mut renamed = small();
    renamed.name = "small2".into();
    assert_ne!(
        router(&renamed).unwrap().specialization_key().unwrap(),
        small_key,
        "a different package is a different specialization"
    );

    // `big` is the configuration of `floo_synth_nw_pkg`, whose link widths
    // Slang reports as 116 (req), 100 (rsp) and 1204 (wide) bits, with a
    // 5-bit node ID (third_party router survey, issue #1).
    assert_eq!(width(&big_router, "id_i"), 5);
    for (port, bits) in [
        ("floo_req_i", 116),
        ("floo_req_o", 116),
        ("floo_rsp_i", 100),
        ("floo_rsp_o", 100),
        ("floo_wide_i", 1204),
        ("floo_wide_o", 1204),
    ] {
        assert_eq!(width(&big_router, port), 5 * bits, "{port}");
    }
    // Narrower data and coordinates and a single wide channel shrink every
    // link of `small`.
    assert_eq!(width(&small_router, "id_i"), 3);
    for port in ["floo_req_i", "floo_rsp_i", "floo_wide_i"] {
        assert!(
            width(&small_router, port) < width(&big_router, port),
            "{port}"
        );
    }
    let pkg_type =
        |r: &topstitch::ModDef, port: &str| r.get_port(port).port_type().unwrap().to_string();
    assert!(pkg_type(&small_router, "floo_wide_i").contains("small_pkg::floo_wide_t"));
    assert!(pkg_type(&big_router, "floo_wide_i").contains("big_pkg::floo_wide_t"));
}

fn check_mesh(cfg: &FlooConfig) -> String {
    let mesh = mesh_2x2(cfg);
    let emitted = mesh.emit(EmitOptions::default());
    let manifest = mesh.manifest();
    let top = format!("{}_mesh_2x2", cfg.name);
    if let Err(slang) = elaborate_manifest(&manifest, &emitted, &top) {
        panic!("Slang rejects the emitted mesh:\n{emitted}\n{slang}");
    }

    // The configuration package and the router are built from the
    // manifest, and every router is the one specialization.
    let pkg = cfg_pkg_file(cfg);
    assert!(manifest.sources.contains(&pkg), "{manifest:#?}");
    assert!(
        manifest
            .sources
            .iter()
            .any(|s| s.ends_with("floonoc/hw/floo_nw_router.sv"))
    );
    let key = router(cfg).unwrap().specialization_key().unwrap();
    let paths: Vec<&str> = manifest
        .specializations
        .iter()
        .map(|(path, hash)| {
            assert_eq!(hash, &key.short_hash());
            path.as_str()
        })
        .collect();
    let expected: Vec<String> = ["x0_y0", "x0_y1", "x1_y0", "x1_y1"]
        .iter()
        .map(|xy| format!("{top}.i_router_{xy}"))
        .collect();
    assert_eq!(paths, expected);
    let pkg_name = cfg.pkg_name();
    assert_eq!(
        emitted
            .matches(&format!(".floo_wide_t({pkg_name}::floo_wide_t)"))
            .count(),
        4
    );
    emitted
}

/// `id_i` is `{x, y, port_id}`, most significant first: its width and value.
fn id_value(cfg: &FlooConfig, x: u32, y: u32) -> (u32, u32) {
    (
        cfg.x_bits + cfg.y_bits + 1,
        (x << (cfg.y_bits + 1)) | (y << 1),
    )
}

/// The width and value of a `W'hX` literal.
fn literal(text: &str) -> (u32, u32) {
    let (width, hex) = text
        .split_once("'h")
        .unwrap_or_else(|| panic!("{text} is not a hex literal"));
    (
        width.parse().unwrap(),
        u32::from_str_radix(&hex.replace('_', ""), 16).unwrap(),
    )
}

#[test]
fn small_mesh_elaborates() {
    let cfg = small();
    let emitted = check_mesh(&cfg);
    for (x, y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        assert_eq!(
            literal(connection(&emitted, &format!("i_router_x{x}_y{y}"), "id_i")),
            id_value(&cfg, x, y)
        );
    }
}

#[test]
fn big_mesh_elaborates() {
    let cfg = big();
    let emitted = check_mesh(&cfg);
    for (x, y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        assert_eq!(
            literal(connection(&emitted, &format!("i_router_x{x}_y{y}"), "id_i")),
            id_value(&cfg, x, y)
        );
    }
    // x = 1 in the upper two bits, above 2-bit y and the port ID.
    assert_eq!(id_value(&cfg, 1, 0), (5, 0b01000));
}

/// The emitted links of `small`: element `d` of a `floo_req_i` (most
/// significant element first in the concatenation) is element `opposite(d)`
/// of the neighbour's `floo_req_o`, mesh edges are zero and the local port
/// is the exported top port.
#[test]
fn small_mesh_links_follow_xy_coordinates() {
    let cfg = small();
    let emitted = mesh_2x2(&cfg).emit(EmitOptions::default());
    let req = width(&router(&cfg).unwrap(), "floo_req_i") / 5;
    let element = |net: &str, i: usize| format!("{net}[{}:{}]", (i + 1) * req - 1, i * req);
    let zero = |n: usize| format!("{}'h", n * req);
    // x0_y0: North (0) from x0_y1's South (2), East (1) from x1_y0's West
    // (3); West and South at the mesh edge.
    let x0_y0 = connection(&emitted, "i_router_x0_y0", "floo_req_i");
    assert!(
        x0_y0.starts_with(&format!("{{x0_y0_p4_floo_req_i, {}", zero(2))),
        "{x0_y0}"
    );
    assert!(
        x0_y0.ends_with(&format!(
            "{}, {}}}",
            element("i_router_x1_y0_floo_req_o", 3),
            element("i_router_x0_y1_floo_req_o", 2)
        )),
        "{x0_y0}"
    );
    // x1_y1: West (3) from x0_y1's East (1), South (2) from x1_y0's North
    // (0); East and North at the mesh edge.
    let x1_y1 = connection(&emitted, "i_router_x1_y1", "floo_req_i");
    assert!(
        x1_y1.starts_with(&format!(
            "{{x1_y1_p4_floo_req_i, {}, {}, {}",
            element("i_router_x0_y1_floo_req_o", 1),
            element("i_router_x1_y0_floo_req_o", 0),
            zero(2)
        )),
        "{x1_y1}"
    );
}

#[test]
fn invalid_configuration_is_rejected() {
    let mut cfg = small();
    cfg.name = "1bad".into();
    assert!(cfg.validate().unwrap_err().contains("identifier"));
    let mut cfg = small();
    cfg.wide.data_width = 60;
    assert!(cfg.validate().unwrap_err().contains("multiple of 8"));
}
