// SPDX-License-Identifier: Apache-2.0

//! Physical design: geometry, shapes, edges, tracks, pin and instance
//! placement, LEF/DEF generation and parsing, physical validators, and the
//! physical methods of the model types (`mod_def`, `mod_inst`, `port`,
//! `port_slice`, `intf`, `io`).

pub(crate) mod def_parse;
pub(crate) mod edges;
pub(crate) mod geometry;
pub(crate) mod intf;
pub(crate) mod io;
pub(crate) mod lef_parse;
pub mod lefdef;
pub(crate) mod lefdef_io;
pub(crate) mod mod_def;
pub(crate) mod mod_inst;
pub(crate) mod pins;
pub(crate) mod placement;
pub(crate) mod port;
pub(crate) mod port_slice;
pub(crate) mod shape;
pub(crate) mod tracks;
pub(crate) mod validate;
