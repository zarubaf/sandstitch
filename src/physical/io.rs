// SPDX-License-Identifier: Apache-2.0

//! LEF/DEF pin directions of [`IO`].

use crate::IO;

impl IO {
    pub fn to_def_direction(&self) -> String {
        match self {
            IO::Input(_) => "INPUT",
            IO::Output(_) => "OUTPUT",
            IO::InOut(_) => "INOUT",
        }
        .to_string()
    }

    pub fn to_lef_direction(&self) -> String {
        match self {
            IO::Input(_) => "INPUT",
            IO::Output(_) => "OUTPUT",
            IO::InOut(_) => "INOUT",
        }
        .to_string()
    }
}
