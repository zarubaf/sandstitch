// SPDX-License-Identifier: Apache-2.0

//! Two router configurations import as distinct specializations, and a 2x2
//! mesh of each emits RTL that Slang elaborates with the build manifest and
//! strict width warnings.

use std::io::Write as _;
use std::process::Command;

use floonoc::*;
use std::path::Path;

use topstitch::{EmitOptions, IO, Manifest, ModDef, ModInst};

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

/// A directory for the configuration packages, deleted when the test ends.
fn out_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn configurations_are_distinct_specializations() {
    let dir = out_dir();
    let small_router = router(&small(), dir.path()).unwrap();
    let big_router = router(&big(), dir.path()).unwrap();
    let small_key = small_router.specialization_key().unwrap();
    let big_key = big_router.specialization_key().unwrap();
    assert_ne!(small_key, big_key);
    assert_eq!(
        router(&small(), dir.path())
            .unwrap()
            .specialization_key()
            .unwrap(),
        small_key,
        "an equal configuration is the same specialization"
    );
    let mut renamed = small();
    renamed.name = "small2".into();
    assert_ne!(
        router(&renamed, dir.path())
            .unwrap()
            .specialization_key()
            .unwrap(),
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

    // The package lives in the caller's directory.
    let pkg = cfg_pkg_file(&small(), dir.path());
    assert_eq!(pkg.parent(), Some(dir.path()));
    assert_eq!(std::fs::read_to_string(&pkg).unwrap(), cfg_pkg(&small()));
}

const ROUTERS: [(u32, u32); 4] = [(0, 0), (0, 1), (1, 0), (1, 1)];

fn check_mesh(cfg: &FlooConfig) -> String {
    let dir = out_dir();
    let mesh = mesh_2x2(cfg, dir.path());
    let emitted = mesh.emit(EmitOptions::default());
    let manifest = mesh.manifest();
    let top = format!("{}_mesh_2x2", cfg.name);
    if let Err(slang) = elaborate_manifest(&manifest, &emitted, &top) {
        panic!("Slang rejects the emitted mesh:\n{emitted}\n{slang}");
    }

    // The configuration package and the router are built from the
    // manifest, and every router is the one specialization.
    let pkg = cfg_pkg_file(cfg, dir.path());
    assert!(manifest.sources.contains(&pkg), "{manifest:#?}");
    assert!(
        manifest
            .sources
            .iter()
            .any(|s| s.ends_with("floonoc/hw/floo_nw_router.sv"))
    );
    let key = router(cfg, dir.path())
        .unwrap()
        .specialization_key()
        .unwrap();
    let paths: Vec<&str> = manifest
        .specializations
        .iter()
        .map(|(path, hash)| {
            assert_eq!(hash, &key.short_hash());
            path.as_str()
        })
        .collect();
    let expected: Vec<String> = ROUTERS
        .iter()
        .map(|(x, y)| format!("{top}.i_router_x{x}_y{y}"))
        .collect();
    assert_eq!(paths, expected);
    let pkg_name = cfg.pkg_name();
    assert_eq!(
        emitted
            .matches(&format!(".floo_wide_t({pkg_name}::floo_wide_t)"))
            .count(),
        4
    );
    for (x, y) in ROUTERS {
        assert_eq!(
            literal(connection(&emitted, &format!("i_router_x{x}_y{y}"), "id_i")),
            id_value(cfg, x, y)
        );
    }
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
    check_mesh(&small());
}

#[test]
fn big_mesh_elaborates() {
    let cfg = big();
    check_mesh(&cfg);
    // x = 1 in the upper two bits, above 2-bit y and the port ID.
    assert_eq!(id_value(&cfg, 1, 0), (5, 0b01000));
}

/// Two local ports per router, both exported.
#[test]
fn six_route_mesh_elaborates() {
    let mut cfg = small();
    cfg.name = "six".into();
    cfg.num_routes = 6;
    let emitted = check_mesh(&cfg);
    for (x, y) in ROUTERS {
        for p in [4, 5] {
            for (dir, ch, suffix) in [("input", "req", "i"), ("output", "wide", "o")] {
                let decl = format!("{dir} six_pkg::floo_{ch}_t x{x}_y{y}_p{p}_floo_{ch}_{suffix}");
                assert!(
                    emitted
                        .lines()
                        .any(|l| l.trim().trim_end_matches(',') == decl),
                    "{decl}"
                );
            }
        }
    }
}

/// What feeds one element of a router's link input.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// Zero (a mesh edge).
    Zero,
    /// Element `index` of another router's link output net.
    Element { net: String, index: usize },
    /// A top-level port (a local link).
    Port(String),
}

/// Splits the concatenation `{a, b, ...}` driving an input port into one
/// [`Source`] per element of `elem` bits, element 0 first. Adjacent
/// elements may share one item (a merged zero literal or a slice of
/// several elements).
fn elements(concat: &str, elem: usize) -> Vec<Source> {
    let items = concat
        .strip_prefix('{')
        .and_then(|c| c.strip_suffix('}'))
        .unwrap_or_else(|| panic!("{concat} is not a concatenation"));
    let mut out = Vec::new();
    for item in items.split(", ").collect::<Vec<_>>().into_iter().rev() {
        if let Some((width, hex)) = item.split_once("'h") {
            let width: usize = width.parse().unwrap();
            assert!(hex.chars().all(|c| c == '0' || c == '_'), "{item}");
            assert_eq!(width % elem, 0, "{item}");
            out.extend((0..width / elem).map(|_| Source::Zero));
        } else if let Some((net, range)) = item.split_once('[') {
            let (msb, lsb) = range.strip_suffix(']').unwrap().split_once(':').unwrap();
            let (msb, lsb): (usize, usize) = (msb.parse().unwrap(), lsb.parse().unwrap());
            assert_eq!(lsb % elem, 0, "{item}");
            assert_eq!((msb + 1) % elem, 0, "{item}");
            out.extend((lsb / elem..=msb / elem).map(|index| Source::Element {
                net: net.to_string(),
                index,
            }));
        } else {
            out.push(Source::Port(item.to_string()));
        }
    }
    out
}

/// Every link input of every router of `small`, for the request, response
/// and wide channels: direction `d` comes from the neighbour's output
/// element `opposite(d)` (North 0, East 1, South 2, West 3), mesh edges
/// are zero and the local port (4) is the exported top port.
#[test]
fn small_mesh_links_follow_xy_coordinates() {
    let cfg = small();
    let dir = out_dir();
    let emitted = mesh_2x2(&cfg, dir.path()).emit(EmitOptions::default());
    let r = router(&cfg, dir.path()).unwrap();
    for ch in ["req", "rsp", "wide"] {
        let elem = width(&r, &format!("floo_{ch}_i")) / 5;
        for (x, y) in ROUTERS {
            let neighbour = |nx: u32, ny: u32, index: usize| Source::Element {
                net: format!("i_router_x{nx}_y{ny}_floo_{ch}_o"),
                index,
            };
            let expected = vec![
                // North: (x, y+1) through its South.
                if y == 0 {
                    neighbour(x, 1, 2)
                } else {
                    Source::Zero
                },
                // East: (x+1, y) through its West.
                if x == 0 {
                    neighbour(1, y, 3)
                } else {
                    Source::Zero
                },
                // South: (x, y-1) through its North.
                if y == 1 {
                    neighbour(x, 0, 0)
                } else {
                    Source::Zero
                },
                // West: (x-1, y) through its East.
                if x == 1 {
                    neighbour(0, y, 1)
                } else {
                    Source::Zero
                },
                Source::Port(format!("x{x}_y{y}_p4_floo_{ch}_i")),
            ];
            let inst = format!("i_router_x{x}_y{y}");
            let concat = connection(&emitted, &inst, &format!("floo_{ch}_i"));
            assert_eq!(elements(concat, elem), expected, "{inst}.floo_{ch}_i");
        }
    }
}

#[test]
fn invalid_configuration_is_rejected() {
    let mut cfg = small();
    cfg.name = "1bad".into();
    assert!(cfg.validate().unwrap_err().contains("identifier"));
    let mut cfg = small();
    cfg.wide.data_width = 60;
    assert!(cfg.validate().unwrap_err().contains("multiple of 8"));
    // A 1-bit port ID addresses two local ports after the four directions.
    for (routes, ok) in [(4, false), (5, true), (6, true), (7, false)] {
        let mut cfg = small();
        cfg.num_routes = routes;
        assert_eq!(cfg.validate().is_ok(), ok, "num_routes {routes}");
    }
}

#[test]
#[should_panic(expected = "num_routes 7 is outside 5..=6")]
fn mesh_rejects_unroutable_port_count() {
    let mut cfg = small();
    cfg.num_routes = 7;
    mesh_2x2(&cfg, out_dir().path());
}

/// The mesh wired with the router link interfaces emits exactly the RTL of
/// the same mesh wired element by element.
#[test]
fn link_interfaces_match_hand_wiring() {
    let mut six = small();
    six.name = "six".into();
    six.num_routes = 6;
    for cfg in [small(), big(), six] {
        let dir = out_dir();
        let by_links = mesh_2x2(&cfg, dir.path()).emit(EmitOptions::default());
        let by_hand = hand_wired_mesh_2x2(&cfg, dir.path()).emit(EmitOptions::default());
        assert_eq!(by_links, by_hand, "{}", cfg.name);
    }
}

const CHANNELS: [&str; 3] = ["req", "rsp", "wide"];

/// Connects the `dir` link of `a` to the `opposite` link of `b`, in both
/// directions: `a.floo_<ch>_o[dir]` drives `b.floo_<ch>_i[opposite]` and
/// `b.floo_<ch>_o[opposite]` drives `a.floo_<ch>_i[dir]`, for the request,
/// response and wide channels. Each link struct carries its own ready.
fn hand_link(a: &ModInst, dir: Direction, b: &ModInst, opposite: Direction) {
    for ch in CHANNELS {
        let (o, i) = (format!("floo_{ch}_o"), format!("floo_{ch}_i"));
        a.get_port(&o)
            .index(dir as usize)
            .connect(&b.get_port(&i).index(opposite as usize));
        b.get_port(&o)
            .index(opposite as usize)
            .connect(&a.get_port(&i).index(dir as usize));
    }
}

/// `mesh_2x2` as it was wired by hand before the router link interfaces.
fn hand_wired_mesh_2x2(cfg: &FlooConfig, out_dir: &Path) -> ModDef {
    let router = router(cfg, out_dir).unwrap();
    let top = ModDef::new(format!("{}_mesh_2x2", cfg.name));
    let clk = top.add_port("clk_i", IO::Input(1));
    let rst = top.add_port("rst_ni", IO::Input(1));
    let test = top.add_port("test_enable_i", IO::Input(1));

    let mut routers = Vec::new();
    for x in 0..2usize {
        let mut column = Vec::new();
        for y in 0..2usize {
            let r = top.instantiate(&router, Some(&format!("i_router_x{x}_y{y}")), None);
            clk.connect(&r.get_port("clk_i"));
            rst.connect(&r.get_port("rst_ni"));
            test.connect(&r.get_port("test_enable_i"));
            let id = r.get_port("id_i");
            id.field("x").tieoff(x);
            id.field("y").tieoff(y);
            id.field("port_id").tieoff(0);
            r.get_port("id_route_map_i").tieoff(0);
            for side in ["wide", "narrow"] {
                r.get_port(format!("offload_{side}_rsp_i")).tieoff(0);
                r.get_port(format!("offload_{side}_req_o")).unused();
            }
            for p in Direction::Eject as usize..cfg.num_routes as usize {
                for ch in CHANNELS {
                    for dir in ["i", "o"] {
                        r.get_port(format!("floo_{ch}_{dir}"))
                            .index(p)
                            .export_as(format!("x{x}_y{y}_p{p}_floo_{ch}_{dir}"));
                    }
                }
            }
            column.push(r);
        }
        routers.push(column);
    }

    hand_link(
        &routers[0][0],
        Direction::East,
        &routers[1][0],
        Direction::West,
    );
    hand_link(
        &routers[0][1],
        Direction::East,
        &routers[1][1],
        Direction::West,
    );
    hand_link(
        &routers[0][0],
        Direction::North,
        &routers[0][1],
        Direction::South,
    );
    hand_link(
        &routers[1][0],
        Direction::North,
        &routers[1][1],
        Direction::South,
    );

    for (x, y) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
        let r = &routers[x][y];
        let x_edge = if x == 0 {
            Direction::West
        } else {
            Direction::East
        };
        let y_edge = if y == 0 {
            Direction::South
        } else {
            Direction::North
        };
        for (dir, ch) in [x_edge, y_edge]
            .into_iter()
            .flat_map(|dir| CHANNELS.map(|ch| (dir, ch)))
        {
            r.get_port(format!("floo_{ch}_i"))
                .index(dir as usize)
                .tieoff(0);
            r.get_port(format!("floo_{ch}_o"))
                .index(dir as usize)
                .unused();
        }
    }
    top
}
