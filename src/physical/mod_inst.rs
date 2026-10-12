// SPDX-License-Identifier: Apache-2.0

//! Physical-design methods of [`ModInst`]: placement, transforms, pins and
//! connection-distance validation.

use crate::{Coordinate, Mat3, ModInst, Orientation, PhysicalPin, Placement};

impl ModInst {
    /// Returns the cumulative placement transform for this instance, combining
    /// every placed level in the hierarchy.
    pub fn get_transform(&self) -> Mat3 {
        let mut total = Mat3::identity();

        for frame in &self.hierarchy {
            let core = frame.mod_def_core.upgrade().unwrap_or_else(|| {
                panic!(
                    "Containing ModDefCore for '{}' has been dropped",
                    frame.inst_name
                )
            });

            let placement = {
                let core_borrowed = core.read();
                core_borrowed.inst_placements.get(&frame.inst_name).copied()
            };

            if let Some(placement) = placement {
                total = &total * &placement.transform();
            }
        }

        total
    }

    /// Validates connection distances for this instance only. This does not
    /// descend into child instances.
    pub fn validate_connection_distances(&self) {
        let self_mod_def = self.get_mod_def();
        let self_mod_def_core_borrowed = self_mod_def.core.read();

        let self_transform = self.get_transform();

        for (self_port_name, self_io) in self_mod_def_core_borrowed.ports.iter() {
            let self_port = self.get_port(self_port_name);
            let self_width = self_io.width();

            if self_width == 0 {
                continue;
            }

            let physical_pins =
                if let Some(pins) = self_mod_def_core_borrowed.physical_pins.get(self_port_name) {
                    pins
                } else {
                    continue;
                };

            let max_distances = self_mod_def_core_borrowed
                .port_max_distances
                .get(self_port_name);

            if max_distances.is_none()
                && self_mod_def_core_borrowed
                    .default_connection_max_distance
                    .is_none()
            {
                continue;
            }

            for bit in 0..self_width {
                let self_physical_pin = if let Some(physical_pin) =
                    physical_pins.get(bit).and_then(|pin| pin.as_ref())
                {
                    physical_pin
                } else {
                    continue;
                };

                let max_distance = if let Some(max_distances) = max_distances {
                    if let Some(max_distance) = max_distances.get(bit) {
                        *max_distance
                    } else {
                        self_mod_def_core_borrowed.default_connection_max_distance
                    }
                } else {
                    self_mod_def_core_borrowed.default_connection_max_distance
                };

                let max_distance = if let Some(max_distance) = max_distance {
                    max_distance
                } else {
                    continue;
                };

                let self_port_slice = self_port.bit(bit);

                let Some((other_port_slice, manhattan_distance)) = self_port_slice
                    .get_connected_bit_and_distance_with_self_transform(
                        &self_transform,
                        self_physical_pin,
                    )
                else {
                    continue;
                };

                if manhattan_distance > max_distance {
                    panic!(
                        "Distance between {} and {} is {}, exceeding the max specified distance of {}",
                        self_port_slice.debug_string(),
                        other_port_slice.debug_string(),
                        manhattan_distance,
                        max_distance,
                    );
                }
            }
        }
    }

    /// Define a physical pin for this instance. The provided `pin` transform is
    /// interpreted in the parent module's coordinate space.
    pub fn place_pin(&self, port_name: impl AsRef<str>, bit: usize, pin: PhysicalPin) {
        let inverse = self.get_transform().inverse();
        let local_transform = &inverse * &pin.transform;
        let local_pin =
            PhysicalPin::from_transform(pin.layer.clone(), pin.polygon.clone(), local_transform);

        self.get_mod_def().place_pin(port_name, bit, local_pin);
    }

    /// Place this instance at a coordinate with an orientation.
    pub fn place<C: Into<Coordinate>>(&self, coordinate: C, orientation: Orientation) {
        let core = self.mod_def_core_where_instantiated();
        core.write().inst_placements.insert(
            self.name().to_string(),
            Placement {
                coordinate: coordinate.into(),
                orientation,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IO, ModDef, Polygon};

    #[test]
    fn mod_inst_transform_and_port_coordinate() {
        let leaf = ModDef::new("Leaf");
        leaf.add_port("p", IO::Output(1));
        let pin = PhysicalPin::from_translation(
            "M1",
            Polygon::from_width_height(1, 1),
            Coordinate { x: 2, y: 3 },
        );
        leaf.place_pin("p", 0, pin);

        let mid = ModDef::new("Mid");
        let leaf_inst = mid.instantiate(&leaf, Some("leaf"), None);
        leaf_inst.place((5, 0), Orientation::R90);

        let top = ModDef::new("Top");
        let mid_inst = top.instantiate(&mid, Some("mid"), None);
        mid_inst.place((10, -2), Orientation::R0);

        let leaf_from_top = mid_inst.get_instance("leaf");
        let total_transform = leaf_from_top.get_transform();

        let pin_world = leaf_from_top.get_port("p").bit(0).get_coordinate();
        assert_eq!(
            pin_world,
            Coordinate { x: 2, y: 3 }.apply_transform(&total_transform)
        );
        assert_eq!(pin_world, Coordinate { x: 12, y: 0 });

        let recovered_local = pin_world.apply_transform(&total_transform.inverse());
        assert_eq!(recovered_local, Coordinate { x: 2, y: 3 });

        // Re-place the pin through the instance using parent-space coordinates.
        let polygon = Polygon::from_width_height(1, 1);
        let world_pin = PhysicalPin::from_translation("M1", polygon.clone(), pin_world);
        leaf_from_top.place_pin("p", 0, world_pin);

        // The underlying module stores pins in local coordinates.
        let local_coord = leaf.get_port("p").bit(0).get_coordinate();
        assert_eq!(local_coord, Coordinate { x: 2, y: 3 });
    }

    #[test]
    fn mod_inst_place_pin_inverts_transform() {
        let child = ModDef::new("Child");
        child.add_port("x", IO::Output(1));

        let parent = ModDef::new("Parent");
        let child_inst = parent.instantiate(&child, Some("c"), None);
        child_inst.place((10, 5), Orientation::R180);

        let world_coord = Coordinate { x: 8, y: 7 };
        let polygon = Polygon::from_width_height(2, 3);
        let world_pin = PhysicalPin::from_translation("M2", polygon.clone(), world_coord);
        child_inst.place_pin("x", 0, world_pin);

        // The stored pin should reside in child-local space.
        let core = child.core.read();
        let pins = core.physical_pins.get("x").unwrap();
        let stored_pin = pins[0].as_ref().unwrap();
        let expected_local = world_coord.apply_transform(&child_inst.get_transform().inverse());
        assert_eq!(stored_pin.translation(), expected_local);
    }
}
