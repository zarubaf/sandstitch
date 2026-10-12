// SPDX-License-Identifier: Apache-2.0

//! The configuration of a FlooNoC narrow/wide network and the SystemVerilog
//! package that defines its types.

use std::fmt::Write as _;

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
        }
    }

    /// The name of the configuration package.
    pub fn pkg_name(&self) -> String {
        format!("{}_pkg", self.name)
    }

    /// Checks what the package and the router need: an identifier name,
    /// non-zero widths and byte-multiple data widths.
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
        if self.num_routes == 0 {
            return Err(format!("{}: num_routes must be non-zero", self.name));
        }
        Ok(())
    }
}

fn axi_cfg(axi: &AxiCfg) -> String {
    format!(
        "'{{\n    AddrWidth: {},\n    DataWidth: {},\n    UserWidth: {},\n    InIdWidth: {},\n    OutIdWidth: {}\n  }}",
        axi.addr_width, axi.data_width, axi.user_width, axi.in_id_width, axi.out_id_width
    )
}

/// The configuration package `<name>_pkg` of `cfg`: the node ID, header and
/// narrow/wide link types built with FlooNoC's typedef macros, as
/// `floo_synth_nw_pkg` does, and the constants the router is parameterized
/// with (`AxiCfgN`, `AxiCfgW`, `RouteAlgo`, `WideRwDecouple`, `VcImpl`).
/// Needs `floo_pkg`, `cc_pkg` and the `axi`/`floo_noc` include directories.
///
/// Panics if `cfg` is invalid ([`FlooConfig::validate`]).
pub fn cfg_pkg(cfg: &FlooConfig) -> String {
    if let Err(e) = cfg.validate() {
        panic!("invalid FlooNoC configuration: {e}");
    }
    let mut s = String::new();
    let pkg = cfg.pkg_name();
    writeln!(s, "// SPDX-License-Identifier: Apache-2.0").unwrap();
    writeln!(
        s,
        "// Generated by the floonoc design package for `{}`.",
        cfg.name
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, "`include \"axi/typedef.svh\"").unwrap();
    writeln!(s, "`include \"floo_noc/typedef.svh\"").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "package {pkg};").unwrap();
    writeln!(
        s,
        "  localparam floo_pkg::axi_cfg_t AxiCfgN = {};",
        axi_cfg(&cfg.narrow)
    )
    .unwrap();
    writeln!(
        s,
        "  localparam floo_pkg::axi_cfg_t AxiCfgW = {};",
        axi_cfg(&cfg.wide)
    )
    .unwrap();
    writeln!(
        s,
        "  localparam floo_pkg::route_algo_e RouteAlgo = floo_pkg::XYRouting;"
    )
    .unwrap();
    writeln!(
        s,
        "  localparam floo_pkg::wide_rw_decouple_e WideRwDecouple = {};",
        cfg.wide_rw_decouple.sv()
    )
    .unwrap();
    writeln!(
        s,
        "  localparam floo_pkg::vc_impl_e VcImpl = {};",
        cfg.vc_impl.sv()
    )
    .unwrap();
    writeln!(
        s,
        "  localparam int unsigned NumVirtualChannels = (WideRwDecouple == floo_pkg::None) ? 1 : 2;"
    )
    .unwrap();
    writeln!(
        s,
        "  localparam int unsigned NumWidePhysChannels = (WideRwDecouple == floo_pkg::Phys) ? 2 : 1;"
    )
    .unwrap();
    writeln!(s).unwrap();
    writeln!(s, "  typedef logic [{}:0] x_bits_t;", cfg.x_bits - 1).unwrap();
    writeln!(s, "  typedef logic [{}:0] y_bits_t;", cfg.y_bits - 1).unwrap();
    s += "  `FLOO_TYPEDEF_XY_NODE_ID_T(id_t, x_bits_t, y_bits_t, logic)\n";
    s += "  `FLOO_TYPEDEF_HDR_T(hdr_t, id_t, id_t, floo_pkg::nw_ch_e, logic)\n";
    s += "  `FLOO_TYPEDEF_AXI_FROM_CFG(axi_narrow, AxiCfgN)\n";
    s += "  `FLOO_TYPEDEF_AXI_FROM_CFG(axi_wide, AxiCfgW)\n";
    s += "  `FLOO_TYPEDEF_NW_CHAN_ALL(axi, req, rsp, wide, axi_narrow_in, axi_wide_in,\n";
    s += "      AxiCfgN, AxiCfgW, hdr_t)\n";
    s += "  `FLOO_TYPEDEF_NW_VIRT_CHAN_LINK_ALL(req, rsp, wide, req, rsp, wide,\n";
    s += "      NumVirtualChannels, NumWidePhysChannels)\n";
    writeln!(s, "endpackage").unwrap();
    s
}
