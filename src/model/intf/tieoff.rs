// SPDX-License-Identifier: Apache-2.0

use crate::PortKind;
use num_bigint::BigInt;

use crate::{IO, Intf, Port, PortSlice};

impl Intf {
    /// Ties off driven signals on this interface to the given constant value. A
    /// "driven signal" is an input of a module instance or an output of a
    /// module definition; it's a signal that would appear on the left hand side
    /// of a Verilog `assign` statement.
    #[track_caller]
    pub fn tieoff<T: Into<BigInt> + Clone>(&self, value: T) {
        for (_, port_slice) in self.members() {
            match port_slice {
                PortSlice {
                    port:
                        Port {
                            kind: PortKind::ModDef { .. },
                        },
                    ..
                } => {
                    if let IO::Output(_) = port_slice.port.io() {
                        port_slice.tieoff(value.clone());
                    }
                }
                PortSlice {
                    port:
                        Port {
                            kind: PortKind::ModInst { .. },
                        },
                    ..
                } => {
                    if let IO::Input(_) = port_slice.port.io() {
                        port_slice.tieoff(value.clone());
                    }
                }
            }
        }
    }

    /// Marks unused driving signals on this interface. A "driving signal" is an
    /// output of a module instance or an input of a module definition; it's a
    /// signal that would appear on the right hand side of a Verilog `assign`
    /// statement.
    #[track_caller]
    pub fn unused(&self) {
        for (_, port_slice) in self.members() {
            match port_slice {
                PortSlice {
                    port:
                        Port {
                            kind: PortKind::ModDef { .. },
                        },
                    ..
                } => {
                    if let IO::Input(_) | IO::InOut(_) = port_slice.port.io() {
                        port_slice.unused();
                    }
                }
                PortSlice {
                    port:
                        Port {
                            kind: PortKind::ModInst { .. },
                        },
                    ..
                } => {
                    if let IO::Output(_) | IO::InOut(_) = port_slice.port.io() {
                        port_slice.unused();
                    }
                }
            }
        }
    }

    #[track_caller]
    pub fn unused_and_tieoff<T: Into<BigInt> + Clone>(&self, value: T) {
        self.unused();
        self.tieoff(value);
    }
}
