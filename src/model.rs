// SPDX-License-Identifier: Apache-2.0

//! Structural model: module definitions and instances, ports and port
//! slices, interfaces, connections and parameters. Physical-design methods
//! of these types live in `crate::physical`.

pub(crate) mod connection;
pub(crate) mod funnel;
pub(crate) mod intf;
pub(crate) mod io;
pub(crate) mod metadata;
pub(crate) mod mod_def;
pub(crate) mod mod_inst;
pub(crate) mod package;
pub(crate) mod parameter;
pub(crate) mod pipeline;
pub(crate) mod port;
pub(crate) mod port_slice;
pub(crate) mod port_type;
pub(crate) mod usage;
