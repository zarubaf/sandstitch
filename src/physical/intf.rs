// SPDX-License-Identifier: Apache-2.0

//! Physical-design methods of [`Intf`]: tracks, pin placement and maximum
//! connection distance.

use std::collections::HashSet;

use crate::model::intf::crossover::find_crossover_matches;
use crate::{Intf, ModDef, PhysicalPin, PinPlacementError, PortSlice};

impl Intf {
    pub fn set_max_distance(&self, max_distance: Option<i64>) {
        for (_, port_slice) in self.get_port_slices() {
            port_slice.set_max_distance(max_distance);
        }
    }

    /// Block tracks for the existing physical pins of every bit in this interface.
    /// Uses the same additive reservations as [`ModDef::block_tracks_for_pin`];
    /// overlapping member slices are allowed. An empty definition interface is a no-op.
    /// Returns an error for instance interfaces, missing pins, or missing track setup.
    /// If any member bit fails, no reservations are changed.
    pub fn block_tracks(&self) -> Result<(), PinPlacementError> {
        if !matches!(self, Intf::ModDef { .. }) {
            return Err(PinPlacementError::RequiresModDef);
        }
        let slices = self.get_port_slices();
        let module = ModDef {
            core: self.get_mod_def_core(),
        };
        module.block_tracks_for_port_bits(slices.values().flat_map(PortSlice::to_bits))
    }

    /// Places this interface across from another interface, matching functions
    pub fn place_across_from(&self, other: &Intf, allow_mismatch: bool) {
        self.place_across_from_generic(other, allow_mismatch, None::<&[&str]>);
    }

    /// Places this interface across from another interface, matching functions
    /// by name, skipping the specified functions.
    pub fn place_across_from_except<'a, I, T>(
        &self,
        other: &Intf,
        allow_mismatch: bool,
        skip: Option<I>,
    ) where
        I: IntoIterator<Item = &'a T>,
        T: AsRef<str> + 'a,
    {
        self.place_across_from_generic(other, allow_mismatch, skip);
    }

    /// Places this interface across from another interface, matching functions
    /// by name. See [`PortSlice::place_across_from`] for placement behavior.
    fn place_across_from_generic<'a, I, T>(
        &self,
        other: &Intf,
        allow_mismatch: bool,
        skip: Option<I>,
    ) where
        I: IntoIterator<Item = &'a T>,
        T: AsRef<str> + 'a,
    {
        let skip_names = skip.map(|i| i.into_iter().map(|i| i.as_ref()).collect::<HashSet<_>>());

        let is_skipped = |func_name: &str| {
            if let Some(names) = skip_names.as_ref() {
                names.contains(func_name)
            } else {
                false
            }
        };

        let self_ports = self.get_port_slices();
        let other_ports = other.get_port_slices();

        for (func_name, self_port) in &self_ports {
            if is_skipped(func_name) {
                continue;
            }

            if let Some(other_port) = other_ports.get(func_name) {
                self_port.place_across_from(other_port.clone());
            } else if !allow_mismatch {
                panic!(
                    "Interfaces {} and {} have mismatched functions and allow_mismatch is false. Example: function '{}' is present in {} but not in {}.",
                    self.debug_string(),
                    other.debug_string(),
                    func_name,
                    self.debug_string(),
                    other.debug_string()
                );
            }
        }

        if !allow_mismatch {
            for (func_name, _) in &other_ports {
                if is_skipped(func_name) {
                    continue;
                }

                if !self_ports.contains_key(func_name) {
                    panic!(
                        "Interfaces {} and {} have mismatched functions and allow_mismatch is false. Example: function '{}' is present in {} but not in {}",
                        self.debug_string(),
                        other.debug_string(),
                        func_name,
                        other.debug_string(),
                        self.debug_string()
                    );
                }
            }
        }
    }

    /// Places this interface across from ModDef ports it is directly connected
    /// to within the same module.
    pub fn place_across(&self) {
        for (_, port_slice) in self.get_port_slices() {
            port_slice.place_across();
        }
    }

    /// Places this interface based on what has been connected to it.
    pub fn place_abutted(&self) {
        for (_, port_slice) in self.get_port_slices() {
            port_slice.place_abutted();
        }
    }

    /// For each port slice in this interface, trace its connectivity to
    /// determine what existing pin it is connected to, and then place a new
    /// pin for the slice that overlaps the connected pin.
    pub fn place_overlapped(&self, pin: &PhysicalPin) {
        for (_, port_slice) in self.get_port_slices() {
            port_slice.place_overlapped(pin);
        }
    }

    /// Places this interface across from another interface using a crossover
    /// matching pattern. Matched functions are found the same way as crossover;
    /// each pair is placed using place_across_from.
    pub fn place_crossover_from(
        &self,
        other: &Intf,
        pattern_a: impl AsRef<str>,
        pattern_b: impl AsRef<str>,
    ) {
        let self_port_slices = self.get_port_slices();
        let other_port_slices = other.get_port_slices();

        for (self_func_name, other_func_name) in
            find_crossover_matches(self, other, pattern_a, pattern_b)
        {
            let self_port = &self_port_slices[&self_func_name];
            let other_port = &other_port_slices[&other_func_name];
            self_port.place_across_from(other_port.clone());
        }
    }
}
