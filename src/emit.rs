// SPDX-License-Identifier: Apache-2.0

//! SystemVerilog emission.

pub(crate) mod manifest;
mod net_types;

pub(crate) use net_types::declarable;
pub(crate) mod verilog;
