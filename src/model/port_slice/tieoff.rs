// SPDX-License-Identifier: Apache-2.0

use num_bigint::BigInt;

use crate::PortSlice;
use crate::model::connection::connected_item::{Tieoff, Unused};
use crate::model::connection::port_slice::CallSite;
use crate::model::port_slice::{ConvertibleToPortSlice, PortDirectionality};

impl PortSlice {
    /// Ties off this port slice to the given constant value, specified as a
    /// `BigInt` or type that can be converted to a `BigInt`.
    #[track_caller]
    pub fn tieoff<T: Into<BigInt>>(&self, value: T) {
        self.tieoff_at(value.into(), CallSite::here());
    }

    pub(crate) fn tieoff_at(&self, value: BigInt, site: CallSite) {
        self.port
            .get_port_connections_define_if_missing()
            .write()
            .add_user(self.to_port_slice(), Tieoff::new(value, self.width()), site);
    }

    /// Marks this port slice as unused, meaning that if it is an module
    /// instance output or module definition input, validation will not fail if
    /// the slice drives nothing. In fact, validation will fail if the slice
    /// drives anything.
    #[track_caller]
    pub fn unused(&self) {
        self.unused_at(CallSite::here());
    }

    pub(crate) fn unused_at(&self, site: CallSite) {
        self.port
            .get_port_connections_define_if_missing()
            .write()
            .add_user(self.to_port_slice(), Unused::new(), site);
    }

    /// Marks this PortSlice as unused or ties it off to the given value,
    /// depending on the directionality of the port. ModDef Input and
    /// InOut ports are marked as unused, as well as ModInst Output and
    /// InOut ports. ModDef Output and ModInst Input ports are tied off.
    #[track_caller]
    pub fn unused_or_tieoff<T: Into<BigInt>>(&self, value: T) {
        match self.get_directionality() {
            PortDirectionality::Driver | PortDirectionality::InOut => {
                self.unused();
            }
            PortDirectionality::Receiver => {
                self.tieoff(value);
            }
        }
    }
}
