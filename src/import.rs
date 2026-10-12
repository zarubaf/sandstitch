// SPDX-License-Identifier: Apache-2.0

//! SystemVerilog import through Slang: parser configuration, module and
//! port import, parameterization, and package extraction.

pub(crate) mod hierarchy;
pub(crate) mod package;
pub(crate) mod parameterize;
pub(crate) mod parser;
pub(crate) mod parser_cfg;
pub(crate) mod specialize;
pub(crate) mod typed;
