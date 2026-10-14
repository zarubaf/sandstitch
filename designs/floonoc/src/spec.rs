// SPDX-License-Identifier: Apache-2.0

//! What sits at each router of a mesh, and the system package of a mesh:
//! the memory address map and the core-side AXI types.

use std::collections::BTreeSet;

use topstitch::{PackageDef, PackageType, PackageValue};

use crate::FlooConfig;
use crate::config::axi_all_ct_names;

/// One traffic core: the memory it targets, by router coordinates, and its
/// seed, which selects its address and data in the target's window (below
/// 1024).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub x: usize,
    pub y: usize,
    pub seed: u32,
}

/// The endpoint at a router's local port.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// A compute tile with one traffic core per target.
    Compute { targets: Vec<Target> },
    /// A memory tile owning the router's address window.
    Memory,
    /// The local link, exported from the mesh.
    Export,
    /// The local link, tied off and unused.
    Tie,
}

/// An X×Y mesh and the endpoint at each router.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MeshSpec {
    nx: usize,
    ny: usize,
    /// Indexed by `x * ny + y`.
    endpoints: Vec<Endpoint>,
}

impl MeshSpec {
    /// An `nx`×`ny` mesh with every local link tied off.
    ///
    /// Panics if a dimension is zero.
    pub fn new(nx: usize, ny: usize) -> Self {
        assert!(
            nx > 0 && ny > 0,
            "mesh dimensions {nx}x{ny} must be non-zero"
        );
        MeshSpec {
            nx,
            ny,
            endpoints: vec![Endpoint::Tie; nx * ny],
        }
    }

    pub fn nx(&self) -> usize {
        self.nx
    }

    pub fn ny(&self) -> usize {
        self.ny
    }

    /// Puts `endpoint` at router (`x`, `y`).
    ///
    /// Panics if the router is outside the mesh.
    #[track_caller]
    pub fn set(&mut self, x: usize, y: usize, endpoint: Endpoint) -> &mut Self {
        let i = self.index(x, y);
        self.endpoints[i] = endpoint;
        self
    }

    /// The endpoint at router (`x`, `y`).
    ///
    /// Panics if the router is outside the mesh.
    #[track_caller]
    pub fn endpoint(&self, x: usize, y: usize) -> &Endpoint {
        &self.endpoints[self.index(x, y)]
    }

    #[track_caller]
    fn index(&self, x: usize, y: usize) -> usize {
        assert!(
            x < self.nx && y < self.ny,
            "router ({x}, {y}) is outside the {}x{} mesh",
            self.nx,
            self.ny
        );
        x * self.ny + y
    }

    /// The coordinates of every router, X-major.
    pub fn routers(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        (0..self.nx).flat_map(move |x| (0..self.ny).map(move |y| (x, y)))
    }

    /// The coordinates of every memory endpoint, X-major.
    pub fn memories(&self) -> Vec<(usize, usize)> {
        self.routers()
            .filter(|&(x, y)| *self.endpoint(x, y) == Endpoint::Memory)
            .collect()
    }

    /// The distinct core counts of the compute tiles.
    pub fn core_counts(&self) -> BTreeSet<usize> {
        self.endpoints
            .iter()
            .filter_map(|e| match e {
                Endpoint::Compute { targets } => Some(targets.len()),
                _ => None,
            })
            .collect()
    }
}

/// The name of the system package of configuration `cfg`.
pub(crate) fn sys_pkg_name(cfg: &FlooConfig) -> String {
    format!("{}_sys_pkg", cfg.name)
}

/// The name of the base address constant of the memory at (`x`, `y`).
pub fn mem_base_name(x: usize, y: usize) -> String {
    format!("MemBase_x{x}_y{y}")
}

/// The base address of the window of router (`x`, `y`): its coordinates at
/// the configuration's address bits.
pub fn window_base(cfg: &FlooConfig, x: usize, y: usize) -> u32 {
    ((x as u32) << cfg.addr_offset_x) | ((y as u32) << cfg.addr_offset_y)
}

/// The prefix of the core-side AXI types of a tile with `n` cores
/// (`<prefix>_req_t`, `<prefix>_rsp_t`, `<prefix>_<ch>_chan_t`,
/// `<prefix>_id_t`).
pub fn core_axi(n: usize) -> String {
    format!("core{n}")
}

/// `$clog2(n)`.
fn clog2(n: usize) -> u32 {
    n.next_power_of_two().trailing_zeros()
}

/// The ID width of each core in a tile of `n` cores: `axi_mux` widens the
/// ID by `$clog2(n)` to the narrow `in_id_width`.
pub fn core_id_width(cfg: &FlooConfig, n: usize) -> Result<u32, String> {
    let widen = clog2(n);
    if n == 0 || cfg.narrow.in_id_width <= widen {
        return Err(format!(
            "{}: {n} cores leave no ID bit of narrow.in_id_width {}",
            cfg.name, cfg.narrow.in_id_width
        ));
    }
    Ok(cfg.narrow.in_id_width - widen)
}

/// The system package `<name>_sys_pkg` of `cfg` and `spec`. Its name
/// derives from the configuration name only, so a design holds one
/// `MeshSpec` per configuration name. It contains:
/// - one `int unsigned` constant `MemBase_x<X>_y<Y>` per memory endpoint,
///   the base of its window ([`window_base`]);
/// - for each core count N > 1, the core-side AXI types: `core<N>_id_t`
///   of `in_id_width - $clog2(N)` bits and `AXI_TYPEDEF_ALL_CT(core<N>,
///   ...)` over the narrow address, data, strobe and user types of the
///   configuration package. A single core uses the configuration
///   package's `axi_narrow_in` types directly.
///
/// Needs the configuration package and the `axi` include directory.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]), a memory lies
/// outside the coordinate bits, or a core count leaves no ID bit.
#[track_caller]
pub fn sys_pkg(cfg: &FlooConfig, spec: &MeshSpec) -> PackageDef {
    if let Err(e) = cfg.validate() {
        panic!("invalid FlooNoC configuration: {e}");
    }
    let fits = |x: usize, y: usize| x < 1 << cfg.x_bits && y < 1 << cfg.y_bits;
    let mut pkg = PackageDef::new(&sys_pkg_name(cfg));
    pkg.include("axi/typedef.svh");
    for (x, y) in spec.memories() {
        assert!(
            fits(x, y),
            "{}: memory ({x}, {y}) is outside the {}x{}-bit coordinates",
            cfg.name,
            cfg.x_bits,
            cfg.y_bits
        );
        pkg.localparam(
            &mem_base_name(x, y),
            PackageType::IntUnsigned,
            PackageValue::Int(window_base(cfg, x, y)),
        );
    }
    let cfg_pkg = cfg.pkg_name();
    for n in spec.core_counts() {
        let width = core_id_width(cfg, n).unwrap_or_else(|e| panic!("{e}"));
        if n == 1 {
            continue;
        }
        let prefix = core_axi(n);
        let id = format!("{prefix}_id_t");
        let req = format!("{prefix}_req_t");
        let rsp = format!("{prefix}_rsp_t");
        let narrow = |t: &str| format!("{cfg_pkg}::axi_narrow_{t}_t");
        let (addr, data, strb, user) = (
            narrow("addr"),
            narrow("data"),
            narrow("strb"),
            narrow("user"),
        );
        let declares = axi_all_ct_names(&prefix);
        let declares: Vec<&str> = declares.iter().map(String::as_str).collect();
        pkg.typedef_logic(&id, width as usize).macro_call(
            "AXI_TYPEDEF_ALL_CT",
            &[&prefix, &req, &rsp, &addr, &id, &data, &strb, &user],
            &declares,
        );
    }
    pkg
}
