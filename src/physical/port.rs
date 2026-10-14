// SPDX-License-Identifier: Apache-2.0

//! Physical-design methods of [`Port`]: pins, tracks, placement and maximum
//! connection distance.

use crate::PortKind;
use crate::{ConvertibleToPortSlice, Coordinate, PhysicalPin, PinPlacementError, Port, PortSlice};

impl Port {
    pub fn set_max_distance(&self, max_distance: Option<i64>) {
        self.to_port_slice().set_max_distance(max_distance);
    }

    pub fn has_physical_pin(&self) -> bool {
        self.to_port_slice().has_physical_pin()
    }

    pub fn get_physical_pin(&self) -> PhysicalPin {
        self.to_port_slice().get_physical_pin()
    }

    /// Block tracks for the existing physical pins of every bit in this port.
    /// Uses the same additive reservations as [`ModDef::block_tracks_for_pin`](crate::ModDef::block_tracks_for_pin).
    /// Returns an error for instance ports, missing pins, or missing track setup.
    /// If any bit fails, no reservations are changed.
    pub fn block_tracks(&self) -> Result<(), PinPlacementError> {
        if !matches!(
            self,
            Port {
                kind: PortKind::ModDef { .. }
            }
        ) {
            return Err(PinPlacementError::RequiresModDef);
        }
        self.get_mod_def_where_declared()
            .block_tracks_for_port_bits(self.to_bits())
    }

    pub fn get_coordinate(&self) -> Coordinate {
        self.get_physical_pin().translation()
    }

    /// Places this port based on what has been connected to it.
    pub fn place_abutted(&self) {
        self.to_port_slice().place_abutted();
    }

    /// Places this port abutted to the specified port or port slice.
    pub fn place_abutted_to<T: ConvertibleToPortSlice>(&self, other: T) {
        self.to_port_slice().place_abutted_to(other);
    }

    /// For each bit in this port, trace its connectivity to determine what
    /// existing pin it is connected to, and then place a new pin for the
    /// port bit that overlaps the connected pin.
    pub fn place_overlapped(&self, pin: &PhysicalPin) {
        self.to_port_slice().place_overlapped(pin);
    }

    /// For each bit `i` in this port, place a new pin that overlaps the `i`-th
    /// bit of `other`.
    pub fn place_overlapped_with<T: ConvertibleToPortSlice>(&self, other: T, pin: &PhysicalPin) {
        self.to_port_slice().place_overlapped_with(other, pin);
    }

    /// Places this port across from the ModDef port it is directly connected
    /// to within the same module.
    pub fn place_across(&self) {
        self.to_port_slice().place_across();
    }

    /// Places this port across from the specified port or port slice.
    pub fn place_across_from<T: ConvertibleToPortSlice>(&self, other: T) {
        self.to_port_slice().place_across_from(other);
    }

    /// See documentation for [`PortSlice::get_connection_distance()`]. This
    /// currently supports only single-bit ports.
    pub fn get_connection_distance(&self) -> Option<i64> {
        self.to_port_slice().get_connection_distance()
    }

    /// See documentation for
    /// [`PortSlice::get_connected_port_slice_and_distance()`]. This currently
    /// supports only single-bit ports.
    pub fn get_connected_port_slice_and_distance(&self) -> Option<(PortSlice, i64)> {
        self.to_port_slice().get_connected_port_slice_and_distance()
    }
}
