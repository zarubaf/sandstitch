// SPDX-License-Identifier: Apache-2.0

//! Structural validation of the model. Physical checks live in
//! `crate::physical::validate`.

pub(crate) mod connection;
pub(crate) mod mod_def;
pub(crate) mod types;
mod unpacked;
