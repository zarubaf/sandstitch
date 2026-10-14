// SPDX-License-Identifier: Apache-2.0

//! The configuration of a FlooNoC narrow/wide network and the SystemVerilog
//! package that defines its types.

use topstitch::{PackageDef, PackageType, PackageValue};

/// Route counts XY routing can address: North, East, South, West and
/// `Eject + port_id` with a 1-bit `port_id`.
const MIN_ROUTES: u32 = 5;
const MAX_ROUTES: u32 = 6;

/// The lowest coordinate address bit: each endpoint's window spans at least
/// the 64 KiB its traffic cores address (`Seed * 64 + 64` with
/// `Seed < 1024`).
const MIN_ADDR_OFFSET: u32 = 16;

/// `floo_pkg::wide_rw_decouple_e`: how wide reads and writes share links.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WideRwDecouple {
    /// One wide link for reads and writes.
    None,
    /// Reads and writes on separate virtual channels of one wide link.
    Vc,
    /// Reads and writes on separate physical wide channels.
    Phys,
}

impl WideRwDecouple {
    fn sv(self) -> &'static str {
        match self {
            WideRwDecouple::None => "floo_pkg::None",
            WideRwDecouple::Vc => "floo_pkg::Vc",
            WideRwDecouple::Phys => "floo_pkg::Phys",
        }
    }
}

/// `floo_pkg::vc_impl_e`: the virtual channel flow control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VcImpl {
    VcNaive,
    VcCredit,
    VcPreemptValid,
}

impl VcImpl {
    fn sv(self) -> &'static str {
        match self {
            VcImpl::VcNaive => "floo_pkg::VcNaive",
            VcImpl::VcCredit => "floo_pkg::VcCredit",
            VcImpl::VcPreemptValid => "floo_pkg::VcPreemptValid",
        }
    }
}

/// One AXI side of the network (`floo_pkg::axi_cfg_t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AxiCfg {
    pub addr_width: u32,
    pub data_width: u32,
    pub user_width: u32,
    pub in_id_width: u32,
    pub out_id_width: u32,
}

/// A narrow/wide FlooNoC network configuration with XY routing, mirroring
/// `floo_synth_params_pkg` and `floo_synth_nw_pkg` in
/// `third_party/floonoc/hw/synth/floo_synth_params_pkg.sv`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FlooConfig {
    /// Names the configuration package `<name>_pkg`; a SystemVerilog
    /// identifier.
    pub name: String,
    /// `AxiCfgN`.
    pub narrow: AxiCfg,
    /// `AxiCfgW`.
    pub wide: AxiCfg,
    /// Width of the X and Y coordinates of a node ID.
    pub x_bits: u32,
    pub y_bits: u32,
    /// Router ports: North, East, South, West, then the local ports
    /// (`floo_pkg::route_direction_e`).
    pub num_routes: u32,
    pub in_fifo_depth: u32,
    pub out_fifo_depth: u32,
    pub wide_rw_decouple: WideRwDecouple,
    pub vc_impl: VcImpl,
    /// The address bit of a destination's X coordinate (`XYAddrOffsetX`):
    /// the network interface routes a request to the router whose
    /// coordinates equal the address bits `[addr_offset_x +: x_bits]` and
    /// `[addr_offset_y +: y_bits]`.
    pub addr_offset_x: u32,
    /// The address bit of a destination's Y coordinate (`XYAddrOffsetY`).
    pub addr_offset_y: u32,
}

impl FlooConfig {
    /// The configuration of `floo_synth_nw_pkg` (48-bit addresses, 64-bit
    /// narrow and 512-bit wide data, physically decoupled wide channels) on
    /// 2x2-bit coordinates, named `name`.
    pub fn synth(name: &str) -> Self {
        FlooConfig {
            name: name.to_string(),
            narrow: AxiCfg {
                addr_width: 48,
                data_width: 64,
                user_width: 5,
                in_id_width: 4,
                out_id_width: 2,
            },
            wide: AxiCfg {
                addr_width: 48,
                data_width: 512,
                user_width: 1,
                in_id_width: 3,
                out_id_width: 1,
            },
            x_bits: 2,
            y_bits: 2,
            num_routes: 5,
            in_fifo_depth: 2,
            out_fifo_depth: 2,
            wide_rw_decouple: WideRwDecouple::Phys,
            vc_impl: VcImpl::VcNaive,
            addr_offset_x: 16,
            addr_offset_y: 20,
        }
    }

    /// The name of the configuration package.
    pub fn pkg_name(&self) -> String {
        format!("{}_pkg", self.name)
    }

    /// Checks what the package, the router and the network interface need:
    /// an identifier name, non-zero widths, byte-multiple data widths, equal
    /// narrow and wide address widths, a route count XY routing can
    /// address, coordinate address bits that start at bit 16 or above (so
    /// that a traffic core's `Seed * 64` offset stays below them), do not
    /// overlap and fit both the narrow address and 32 bits (so each
    /// endpoint's base address is a 32-bit constant), and a virtual channel
    /// implementation other than
    /// [`VcImpl::VcCredit`] (FlooNoC v0.9.0's `floo_vc_arbiter` connects a
    /// pin its `cc_credit_counter` no longer has).
    pub fn validate(&self) -> Result<(), String> {
        let mut chars = self.name.chars();
        let ident = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ident {
            return Err(format!(
                "configuration name `{}` is not a SystemVerilog identifier",
                self.name
            ));
        }
        for (side, axi) in [("narrow", &self.narrow), ("wide", &self.wide)] {
            for (field, value) in [
                ("addr_width", axi.addr_width),
                ("data_width", axi.data_width),
                ("user_width", axi.user_width),
                ("in_id_width", axi.in_id_width),
                ("out_id_width", axi.out_id_width),
            ] {
                if value == 0 {
                    return Err(format!("{}: {side}.{field} must be non-zero", self.name));
                }
            }
            if axi.data_width % 8 != 0 {
                return Err(format!(
                    "{}: {side}.data_width {} is not a multiple of 8",
                    self.name, axi.data_width
                ));
            }
        }
        if self.x_bits == 0 || self.y_bits == 0 {
            return Err(format!("{}: x_bits and y_bits must be non-zero", self.name));
        }
        // XY routing selects North..West or `Eject + port_id`, and `port_id`
        // is one bit: four directions plus one or two local ports.
        if !(MIN_ROUTES..=MAX_ROUTES).contains(&self.num_routes) {
            return Err(format!(
                "{}: num_routes {} is outside {MIN_ROUTES}..={MAX_ROUTES} \
                 (four directions and one or two local ports)",
                self.name, self.num_routes
            ));
        }
        if self.narrow.addr_width != self.wide.addr_width {
            return Err(format!(
                "{}: narrow.addr_width {} differs from wide.addr_width {}",
                self.name, self.narrow.addr_width, self.wide.addr_width
            ));
        }
        // A traffic core writes 64 bytes at `Seed * 64` with `Seed < 1024`:
        // the low 16 address bits belong to the window.
        if self.addr_offset_x < MIN_ADDR_OFFSET {
            return Err(format!(
                "{}: addr_offset_x {} is below {MIN_ADDR_OFFSET} \
                 (a 64 KiB window per endpoint)",
                self.name, self.addr_offset_x
            ));
        }
        if self.addr_offset_x + self.x_bits > self.addr_offset_y {
            return Err(format!(
                "{}: X coordinate bits {}..{} overlap the Y coordinate at bit {}",
                self.name,
                self.addr_offset_x,
                self.addr_offset_x + self.x_bits,
                self.addr_offset_y
            ));
        }
        let y_end = self.addr_offset_y + self.y_bits;
        if y_end > self.narrow.addr_width {
            return Err(format!(
                "{}: Y coordinate bits end at {y_end}, above narrow.addr_width {}",
                self.name, self.narrow.addr_width
            ));
        }
        if y_end > 32 {
            return Err(format!(
                "{}: Y coordinate bits end at {y_end}, above 32 \
                 (endpoint base addresses are 32-bit constants)",
                self.name
            ));
        }
        if self.vc_impl == VcImpl::VcCredit {
            return Err(format!(
                "{}: VcCredit is unsupported: FlooNoC v0.9.0's floo_vc_arbiter \
                 connects cc_credit_counter's former credit_init_i pin",
                self.name
            ));
        }
        Ok(())
    }
}

fn axi_cfg(axi: &AxiCfg) -> PackageValue {
    let field = |name: &str, v: u32| (name.to_string(), PackageValue::Int(v));
    PackageValue::Fields(vec![
        field("AddrWidth", axi.addr_width),
        field("DataWidth", axi.data_width),
        field("UserWidth", axi.user_width),
        field("InIdWidth", axi.in_id_width),
        field("OutIdWidth", axi.out_id_width),
    ])
}

/// `floo_pkg::route_cfg_t` for address-based XY routing without tables: the
/// network interface reads the destination coordinates from the address
/// bits at `addr_offset_x` and `addr_offset_y`. Every field is spelled out.
fn route_cfg(cfg: &FlooConfig) -> PackageValue {
    let int = |name: &str, v: u32| (name.to_string(), PackageValue::Int(v));
    let name = |field: &str, v: &str| (field.to_string(), PackageValue::Name(v.to_string()));
    PackageValue::Fields(vec![
        name("RouteAlgo", "floo_pkg::XYRouting"),
        int("UseIdTable", 0),
        int("XYAddrOffsetX", cfg.addr_offset_x),
        int("XYAddrOffsetY", cfg.addr_offset_y),
        int("IdAddrOffset", 0),
        int("NumSamRules", 0),
        int("NumRoutes", 0),
        name("CollectiveCfg", "floo_pkg::CollectiveDefaultCfg"),
    ])
}

/// A narrow `floo_pkg::chimney_cfg_t`: `floo_pkg::ChimneyDefaultCfg` with
/// the subordinate (`EnSbrPort`) and manager (`EnMgrPort`) ports chosen.
/// `CutRsp` equals the default's, which the wide side uses.
fn chimney_cfg(en_sbr: bool, en_mgr: bool) -> PackageValue {
    let int = |name: &str, v: u32| (name.to_string(), PackageValue::Int(v));
    let no_rob = |name: &str| {
        (
            name.to_string(),
            PackageValue::Name("floo_pkg::NoRoB".into()),
        )
    };
    PackageValue::Fields(vec![
        int("EnSbrPort", en_sbr.into()),
        int("EnMgrPort", en_mgr.into()),
        int("MaxTxns", 32),
        int("MaxUniqueIds", 1),
        int("MaxTxnsPerId", 32),
        no_rob("BRoBType"),
        int("BRoBSize", 0),
        no_rob("RRoBType"),
        int("RRoBSize", 0),
        int("CutAx", 0),
        int("CutRsp", 0),
        int("CutOup", 0),
    ])
}

/// The names `FLOO_TYPEDEF_AXI_FROM_CFG(name, cfg)` declares: the address,
/// ID, user, data and strobe types, and for the `in` (manager) and `out`
/// (subordinate) sides the names of `AXI_TYPEDEF_ALL_CT`.
fn axi_from_cfg_names(name: &str) -> Vec<String> {
    let mut names: Vec<String> = ["addr", "in_id", "out_id", "user", "data", "strb"]
        .iter()
        .map(|t| format!("{name}_{t}_t"))
        .collect();
    for side in ["in", "out"] {
        names.extend(axi_all_ct_names(&format!("{name}_{side}")));
    }
    names
}

/// The names `AXI_TYPEDEF_ALL_CT(name, name_req_t, name_rsp_t, ...)`
/// declares: five channel types, the request and the response.
pub(crate) fn axi_all_ct_names(name: &str) -> Vec<String> {
    [
        "aw_chan", "w_chan", "b_chan", "ar_chan", "r_chan", "req", "rsp",
    ]
    .iter()
    .map(|t| format!("{name}_{t}_t"))
    .collect()
}

/// The configuration package `<name>_pkg` of `cfg`: the node ID, header and
/// narrow/wide link types built with FlooNoC's typedef macros, as
/// `floo_synth_nw_pkg` does, and the constants the router is parameterized
/// with (`AxiCfgN`, `AxiCfgW`, `RouteAlgo`, `WideRwDecouple`, `VcImpl`), and
/// what the network interface needs: the AXI types of both sides,
/// `RouteCfg`, the narrow chimney configurations of compute
/// (`ChimneyCfgNCompute`, manager port only) and memory (`ChimneyCfgNMem`,
/// subordinate port only) endpoints, the wide `ChimneyCfgW`, and
/// `sam_rule_t`. Nothing in it depends on a system's address map.
/// Needs `floo_pkg`, `cc_pkg` and the `axi`/`floo_noc` include directories.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]).
pub fn cfg_pkg(cfg: &FlooConfig) -> PackageDef {
    if let Err(e) = cfg.validate() {
        panic!("invalid FlooNoC configuration: {e}");
    }
    let named = |t: &str| PackageType::Named(t.to_string());
    let name = |v: &str| PackageValue::Name(v.to_string());
    let virtual_channels = if cfg.wide_rw_decouple == WideRwDecouple::None {
        1
    } else {
        2
    };
    let wide_phys_channels = if cfg.wide_rw_decouple == WideRwDecouple::Phys {
        2
    } else {
        1
    };
    let axi_narrow = axi_from_cfg_names("axi_narrow");
    let axi_narrow: Vec<&str> = axi_narrow.iter().map(String::as_str).collect();
    let axi_wide = axi_from_cfg_names("axi_wide");
    let axi_wide: Vec<&str> = axi_wide.iter().map(String::as_str).collect();
    let mut pkg = PackageDef::new(&cfg.pkg_name());
    pkg.include("axi/typedef.svh")
        .include("floo_noc/typedef.svh")
        .localparam(
            "AxiCfgN",
            named("floo_pkg::axi_cfg_t"),
            axi_cfg(&cfg.narrow),
        )
        .localparam("AxiCfgW", named("floo_pkg::axi_cfg_t"), axi_cfg(&cfg.wide))
        .localparam(
            "RouteAlgo",
            named("floo_pkg::route_algo_e"),
            name("floo_pkg::XYRouting"),
        )
        .localparam(
            "WideRwDecouple",
            named("floo_pkg::wide_rw_decouple_e"),
            name(cfg.wide_rw_decouple.sv()),
        )
        .localparam(
            "VcImpl",
            named("floo_pkg::vc_impl_e"),
            name(cfg.vc_impl.sv()),
        )
        .localparam(
            "NumVirtualChannels",
            PackageType::IntUnsigned,
            PackageValue::Int(virtual_channels),
        )
        .localparam(
            "NumWidePhysChannels",
            PackageType::IntUnsigned,
            PackageValue::Int(wide_phys_channels),
        )
        .localparam("RouteCfg", named("floo_pkg::route_cfg_t"), route_cfg(cfg))
        .localparam(
            "ChimneyCfgNCompute",
            named("floo_pkg::chimney_cfg_t"),
            chimney_cfg(false, true),
        )
        .localparam(
            "ChimneyCfgNMem",
            named("floo_pkg::chimney_cfg_t"),
            chimney_cfg(true, false),
        )
        .localparam(
            "ChimneyCfgW",
            named("floo_pkg::chimney_cfg_t"),
            name("floo_pkg::ChimneyDefaultCfg"),
        )
        .typedef_logic("sam_rule_t", 1)
        .typedef_logic("x_bits_t", cfg.x_bits as usize)
        .typedef_logic("y_bits_t", cfg.y_bits as usize)
        .macro_call(
            "FLOO_TYPEDEF_XY_NODE_ID_T",
            &["id_t", "x_bits_t", "y_bits_t", "logic"],
            &["id_t"],
        )
        .macro_call(
            "FLOO_TYPEDEF_HDR_T",
            &["hdr_t", "id_t", "id_t", "floo_pkg::nw_ch_e", "logic"],
            &["hdr_t"],
        )
        .macro_call(
            "FLOO_TYPEDEF_AXI_FROM_CFG",
            &["axi_narrow", "AxiCfgN"],
            &axi_narrow,
        )
        .macro_call(
            "FLOO_TYPEDEF_AXI_FROM_CFG",
            &["axi_wide", "AxiCfgW"],
            &axi_wide,
        )
        .macro_call(
            "FLOO_TYPEDEF_NW_CHAN_ALL",
            &[
                "axi",
                "req",
                "rsp",
                "wide",
                "axi_narrow_in",
                "axi_wide_in",
                "AxiCfgN",
                "AxiCfgW",
                "hdr_t",
            ],
            &[],
        )
        .macro_call(
            "FLOO_TYPEDEF_NW_VIRT_CHAN_LINK_ALL",
            &[
                "req",
                "rsp",
                "wide",
                "req",
                "rsp",
                "wide",
                "NumVirtualChannels",
                "NumWidePhysChannels",
            ],
            &["floo_req_t", "floo_rsp_t", "floo_wide_t"],
        );
    pkg
}
