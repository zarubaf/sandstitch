// SPDX-License-Identifier: Apache-2.0

//! Physical-design methods of [`PortSlice`]: pins, placement relative to
//! connected pins, and maximum connection distance.

use std::sync::Arc;

use indexmap::map::Entry;

use crate::model::connection::connected_item::ConnectedItem;
use crate::{
    ConvertibleToPortSlice, Coordinate, EdgeOrientation, Mat3, PhysicalPin, Port, PortSlice,
};

impl PortSlice {
    fn is_mod_def_port_slice(&self) -> bool {
        matches!(self.port, Port::ModDef { .. })
    }

    /// Places this port based on what has been connected to it.
    pub fn place_abutted(&self) {
        for bit_index in self.lsb..=self.msb {
            let bit = self.port.bit(bit_index);
            if let Some(source) = bit.trace_through_hierarchy() {
                bit.place_abutted_to(source);
            }
        }
    }

    /// Place this single-bit slice by deriving coordinates from `source`,
    /// snapping to the nearest usable edge and track.
    pub fn place_abutted_to<T: ConvertibleToPortSlice>(&self, source: T) {
        self.check_validity();

        let source_slice = source.to_port_slice();
        source_slice.check_validity();

        let width = self.width();
        let source_width = source_slice.width();

        if width != source_width {
            panic!(
                "Width mismatch when placing {} with respect to {}",
                self.debug_string(),
                source_slice.debug_string()
            );
        }

        if width == 1 {
            if self.has_physical_pin() {
                return;
            }

            if !source_slice.has_physical_pin() {
                return;
            }

            let source_pin = source_slice.get_physical_pin();
            self.place_from_physical_pin(&source_pin);
        } else {
            for i in 0..width {
                let self_bit = self.slice_with_offset_and_width(i, 1);
                let source_bit = source_slice.slice_with_offset_and_width(i, 1);
                self_bit.place_abutted_to(source_bit);
            }
        }
    }

    /// For each bit in this port slice, trace its connectivity to determine
    /// what existing pin it is connected to, and then place a new pin for
    /// the port slice bit that overlaps the connected pin.
    pub fn place_overlapped(&self, pin: &PhysicalPin) {
        for bit_index in self.lsb..=self.msb {
            let bit = self.port.bit(bit_index);
            if let Some(source) = bit.trace_through_hierarchy() {
                bit.place_overlapped_with(source, pin);
            }
        }
    }

    /// Places this port slice across from the ModDef port slice it is directly
    /// connected to within the same module.
    pub fn place_across(&self) {
        self.check_validity();

        assert!(
            self.is_mod_def_port_slice(),
            "place_across only operates on a ModDef port slice"
        );

        let Some(connections) = self.get_port_connections() else {
            return;
        };

        // TODO(sherbst) 2026-01-22: operate bitwise for more flexibility
        // TODO(sherbst) 2026-01-22: allow multiple connected ModDef port slices
        // as long as only one has pin locations defined
        let mut placement_source = None;
        for connection in &connections {
            if let ConnectedItem::PortSlice(other) = &connection.other
                && other.is_mod_def_port_slice()
            {
                if placement_source.is_some() {
                    panic!("Ambiguous place-across source for {}", self.debug_string());
                } else {
                    placement_source = Some(other.clone());
                }
            }
        }

        if let Some(placement_source) = placement_source {
            self.place_across_from(placement_source);
        }
    }

    /// For each bit `i` in this port slice, place a new pin that overlaps the
    /// `i`-th bit of `other`.
    pub fn place_overlapped_with<T: ConvertibleToPortSlice>(&self, other: T, pin: &PhysicalPin) {
        self.check_validity();

        let other_slice = other.to_port_slice();
        other_slice.check_validity();

        let width = self.width();
        let other_width = other_slice.width();

        if width != other_width {
            panic!(
                "Width mismatch when placing {} with respect to {}",
                self.debug_string(),
                other_slice.debug_string()
            );
        }

        if width == 1 {
            if self.has_physical_pin() {
                return;
            }

            if !other_slice.has_physical_pin() {
                return;
            }

            let other_pin = other_slice.get_physical_pin();
            self.overlap_with_physical_pin(&other_pin, pin);
        } else {
            for i in 0..width {
                let self_bit = self.slice_with_offset_and_width(i, 1);
                let other_bit = other_slice.slice_with_offset_and_width(i, 1);
                self_bit.place_overlapped_with(other_bit, pin);
            }
        }
    }

    /// Place this single-bit slice on the edge opposite `source`, preserving
    /// the layer and track index.
    pub fn place_across_from<T: ConvertibleToPortSlice>(&self, source: T) {
        self.check_validity();

        let source_slice = source.to_port_slice();
        source_slice.check_validity();

        let width = self.width();
        let source_width = source_slice.width();

        if width != source_width {
            panic!(
                "Width mismatch when placing {} with respect to {}",
                self.debug_string(),
                source_slice.debug_string()
            );
        }

        if width == 1 {
            if self.has_physical_pin() {
                return;
            }

            if !source_slice.has_physical_pin() {
                return;
            }

            let src_mod_def = source_slice.get_mod_def_where_declared();
            let src_mod_def_core = src_mod_def.core;
            let dst_mod_def = self.get_mod_def_where_declared();
            let dst_mod_def_core = dst_mod_def.core;
            if !Arc::ptr_eq(&src_mod_def_core, &dst_mod_def_core) {
                panic!(
                    "place_across_from requires source and target slices to belong to the same module definition"
                );
            }

            let core = src_mod_def_core.read();
            let src_pin = core.get_physical_pin(source_slice.port.name(), source_slice.lsb);
            let src_position = src_pin.translation();

            let dst_edge_idx = core
                .shape
                .as_ref()
                .unwrap()
                .find_opposite_edge(&src_position)
                .unwrap_or_else(|err| panic!("{}", err));
            let dst_edge = core.shape.as_ref().unwrap().get_edge(dst_edge_idx);
            drop(core);

            let dst_coordinate = match dst_edge.orientation() {
                Some(EdgeOrientation::North | EdgeOrientation::South) => {
                    src_position.with_x(dst_edge.a.x)
                }
                Some(EdgeOrientation::East | EdgeOrientation::West) => {
                    src_position.with_y(dst_edge.a.y)
                }
                None => panic!("Edge is not axis-aligned; only rectilinear edges are supported"),
            };

            let edge_rotation = dst_edge.to_pin_transform();
            let translation = Mat3::translate(dst_coordinate.x, dst_coordinate.y);
            let transform = &translation * &edge_rotation;
            let across_pin =
                PhysicalPin::from_transform(&src_pin.layer, src_pin.polygon.clone(), transform);

            PortSlice {
                port: Port::ModDef {
                    mod_def_core: Arc::downgrade(&dst_mod_def_core),
                    name: self.port.name().to_string(),
                },
                msb: self.msb,
                lsb: self.lsb,
            }
            .place_from_physical_pin(&across_pin);
        } else {
            for i in 0..width {
                let self_bit = self.slice_with_offset_and_width(i, 1);
                let source_bit = source_slice.slice_with_offset_and_width(i, 1);
                self_bit.place_across_from(source_bit);
            }
        }
    }

    /// Place a pin for this single-bit slice that overlaps `src_pin`, using the
    /// layer name and shape in `dst_pin`. The layer name in `src_pin` is
    /// ignored, as is the position in `dst_pin`. Only works for single-bit
    /// slices, and it is up to the user to ensure `src_pin` is in the same
    /// global coordinate space as the `ModInst` associated with this port
    /// slice.
    pub fn overlap_with_physical_pin(&self, src_pin: &PhysicalPin, dst_pin: &PhysicalPin) {
        self.check_validity();
        assert!(self.width() == 1, "place_from requires single-bit slices");

        let (target_mod_def, target_transform) = match &self.port {
            Port::ModDef { .. } => (self.get_mod_def(), Mat3::identity()),
            Port::ModInst { .. } => {
                let inst = self
                    .port
                    .get_mod_inst()
                    .expect("Port::ModInst hierarchy cannot be empty");
                (inst.get_mod_def(), inst.get_transform())
            }
        };

        // Calculate the position of the destination pin so that source and destination
        // positions align.
        let inverse_transform = target_transform.inverse();
        let src_position = src_pin.translation().apply_transform(&inverse_transform);
        let dst_position = src_position - dst_pin.translation();

        let port_name = self.port.get_port_name();
        let dst_physical_pin = PhysicalPin::from_transform(
            &dst_pin.layer,
            dst_pin.polygon.clone(),
            dst_position.to_transform(),
        );
        target_mod_def.place_pin(port_name, self.lsb, dst_physical_pin);
    }

    /// Place this single-bit slice using the provided physical pin. The caller
    /// is responsible for ensuring the pin is in the appropriate coordinate
    /// space.
    pub fn place_from_physical_pin(&self, source_pin: &PhysicalPin) {
        // TODO: should this take into account the polygon in the PhysicalPin object?

        self.check_validity();
        assert!(self.width() == 1, "place_from requires single-bit slices");

        let (target_mod_def, target_transform) = match &self.port {
            Port::ModDef { .. } => (self.get_mod_def(), Mat3::identity()),
            Port::ModInst { .. } => {
                let inst = self
                    .port
                    .get_mod_inst()
                    .expect("Port::ModInst hierarchy cannot be empty");
                (inst.get_mod_def(), inst.get_transform())
            }
        };

        let inverse_transform = target_transform.inverse();
        let source_coord_local = source_pin.translation().apply_transform(&inverse_transform);
        let layer_name = &source_pin.layer;

        let Some(shape) = target_mod_def.get_shape() else {
            panic!(
                "Target module {} must have a defined shape to place a pin by abutment.",
                target_mod_def.get_name()
            );
        };

        let Some(track) = target_mod_def.get_track(layer_name) else {
            return;
        };

        let Some(edge_index) = shape.closest_edge_index_where(&source_coord_local, |edge| {
            edge.get_index_range(&track).is_some()
        }) else {
            return;
        };

        let Some(relative_track_index) = target_mod_def.nearest_relative_track_index(
            edge_index,
            layer_name,
            &source_coord_local,
        ) else {
            return;
        };

        let port_name = self.port.get_port_name();
        target_mod_def.place_pin_on_edge_index(
            port_name,
            self.lsb,
            edge_index,
            layer_name,
            relative_track_index,
        );
    }

    pub fn set_max_distance(&self, max_distance: Option<i64>) {
        let port_name = self.port.name().to_string();
        let core_rc = self.port.get_mod_def_core_where_declared();
        let mut core = core_rc.write();
        let core_name = core.name.clone();
        let width = core
            .ports
            .get(&port_name)
            .unwrap_or_else(|| {
                panic!(
                    "Port {} not found in module definition {}",
                    port_name, core.name
                )
            })
            .width();

        if width == 0 {
            return;
        }

        let max_distances = match core.port_max_distances.entry(port_name.clone()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(v) => v.insert(vec![None; width]),
        };

        if max_distances.len() != width {
            panic!(
                "Max distance entries for {}.{} have width {}, expected {}",
                core_name,
                port_name,
                max_distances.len(),
                width
            );
        }

        for bit in max_distances.iter_mut().take(self.msb + 1).skip(self.lsb) {
            *bit = max_distance;
        }
    }

    /// Returns `true` if this port slice has a physical pin defined, `false`
    /// otherwise.
    pub fn has_physical_pin(&self) -> bool {
        let bit = if self.lsb == self.msb {
            self.lsb
        } else {
            panic!(
                "has_physical_pin can only be called on single-bit port slices (called on {})",
                self.debug_string()
            );
        };

        self.port
            .get_mod_def_core_where_declared()
            .read()
            .physical_pins
            .get(self.port.name())
            .and_then(|pins| pins.get(bit).and_then(|pin| pin.as_ref()))
            .is_some()
    }

    /// Returns the physical pin for this slice. For ModDef ports, this is in
    /// the local coordinate space, whereas for ModInst ports, this is with
    /// respect to the coordinate space of the parent module.
    pub fn get_physical_pin(&self) -> PhysicalPin {
        if self.width() != 1 {
            panic!(
                "Port slice {} must be a single bit to compute a pin position",
                self.debug_string()
            );
        }

        match &self.port {
            Port::ModDef { mod_def_core, name } => mod_def_core
                .upgrade()
                .unwrap()
                .read()
                .get_physical_pin(name, self.lsb),
            Port::ModInst { .. } => {
                let mod_inst = self
                    .port
                    .get_mod_inst()
                    .expect("Port::ModInst hierarchy cannot be empty");
                let transform = mod_inst.get_transform();
                let pin = mod_inst
                    .get_mod_def()
                    .get_physical_pin(self.port.name(), self.lsb);

                let combined_transform = &transform * &pin.transform;
                PhysicalPin::from_transform(pin.layer, pin.polygon, combined_transform)
            }
        }
    }

    /// Returns the physical coordinate of this single-bit port slice.
    pub fn get_coordinate(&self) -> Coordinate {
        self.get_physical_pin().translation()
    }

    /// Returns the Manhattan gap between this ModInst port slice's physical
    /// pin and the physical pin it traces to.
    ///
    /// This currently supports only single-bit ModInst port slices. Wider port
    /// slices may be supported in the future, but for now this panics if called
    /// on a multi-bit slice.
    ///
    /// Returns `None` when the bit is tied off, unconnected, has multi-fanout,
    /// hits another trace error, traces to a top-level port, or either side
    /// does not have a physical pin.
    pub fn get_connection_distance(&self) -> Option<i64> {
        self.get_connected_port_slice_and_distance()
            .map(|(_, distance)| distance)
    }

    /// Returns the ModInst port slice connected to this ModInst port slice,
    /// along with the Manhattan gap between their physical pins.
    ///
    /// This currently supports only single-bit ModInst port slices. Wider port
    /// slices may be supported in the future, but for now this panics if called
    /// on a multi-bit slice.
    ///
    /// Returns `None` when the bit is tied off, unconnected, has multi-fanout,
    /// hits another trace error, traces to a top-level port, or either side
    /// does not have a physical pin.
    pub fn get_connected_port_slice_and_distance(&self) -> Option<(PortSlice, i64)> {
        let Some(self_mod_inst) = self.port.get_mod_inst() else {
            panic!(
                "get_connected_port_slice_and_distance only works on ports (or slices of ports) on module instances"
            );
        };

        let self_transform = self_mod_inst.get_transform();

        // TODO(sherbst) 2026-04-30: Support multi-bit port slices by returning a
        // matching-width connected PortSlice and an aggregate distance, such as
        // the maximum bit distance. Decide how to represent cases without a
        // single connected PortSlice covering the full width; one option is to
        // return the PortSlice associated with the longest-distance bit.
        let self_physical_pin = self.get_local_physical_pin()?;
        self.get_connected_bit_and_distance_with_self_transform(&self_transform, &self_physical_pin)
    }

    /// Crate-internal fast path for callers that already know this slice's
    /// instance transform and physical pin, such as per-bit validation loops.
    /// This avoids recomputing `ModInst::get_transform()` and re-reading the
    /// physical pin map for every bit.
    pub(crate) fn get_connected_bit_and_distance_with_self_transform(
        &self,
        self_transform: &Mat3,
        self_physical_pin: &PhysicalPin,
    ) -> Option<(PortSlice, i64)> {
        self.check_validity();
        assert!(
            matches!(self.port, Port::ModInst { .. }),
            "get_connected_bit_and_distance_with_self_transform requires a port slice on a module instance"
        );

        let other = self.trace_through_hierarchy_impl(true)?;
        let other_mod_inst = other.port.get_mod_inst()?;
        let other_transform = other_mod_inst.get_transform();
        let other_physical_pin = other.get_local_physical_pin()?;

        let distance = physical_pin_distance(
            self_transform,
            self_physical_pin,
            &other_transform,
            &other_physical_pin,
        );

        Some((other, distance))
    }

    fn get_local_physical_pin(&self) -> Option<PhysicalPin> {
        assert!(
            self.width() == 1,
            "physical pin lookup currently only supports single-bit port slices (called on {})",
            self.debug_string()
        );

        self.port
            .get_mod_def_core_where_declared()
            .read()
            .physical_pins
            .get(self.port.name())
            .and_then(|pins| pins.get(self.lsb))
            .and_then(|pin| pin.as_ref())
            .cloned()
    }
}

fn physical_pin_distance(
    a_transform: &Mat3,
    a_physical_pin: &PhysicalPin,
    b_transform: &Mat3,
    b_physical_pin: &PhysicalPin,
) -> i64 {
    let a_bbox = a_physical_pin
        .transformed_polygon()
        .apply_transform(a_transform)
        .bbox();
    let b_bbox = b_physical_pin
        .transformed_polygon()
        .apply_transform(b_transform)
        .bbox();

    a_bbox.gap(&b_bbox)
}

#[cfg(test)]
mod tests {
    use crate::{
        Coordinate, IO, ModDef, PhysicalPin, Polygon, TrackDefinition, TrackDefinitions,
        TrackOrientation,
    };

    #[test]
    fn test_is_mod_def_port_slice() {
        let module = ModDef::new("M");
        module.add_port("a", IO::Input(1));
        assert!(module.get_port("a").bit(0).is_mod_def_port_slice());

        let top = ModDef::new("Top");
        let inst = top.instantiate(&module, Some("u_m"), None);
        assert!(!inst.get_port("a").bit(0).is_mod_def_port_slice());
    }

    #[test]
    fn test_has_physical_pin_mod_def() {
        let module = ModDef::new("M");
        module.add_port("a", IO::Input(2));

        let pin_shape = Polygon::from_width_height(1, 1);
        let pin = PhysicalPin::from_translation("M1", pin_shape, Coordinate { x: 1, y: 2 });
        module.place_pin("a", 0, pin);

        assert!(module.get_port("a").bit(0).has_physical_pin());
        assert!(!module.get_port("a").bit(1).has_physical_pin());
    }

    #[test]
    fn test_has_physical_pin_mod_inst() {
        let leaf = ModDef::new("Leaf");
        leaf.add_port("a", IO::Input(1));

        let pin_shape = Polygon::from_width_height(1, 1);
        let pin = PhysicalPin::from_translation("M1", pin_shape, Coordinate { x: 0, y: 0 });
        leaf.place_pin("a", 0, pin);

        let top = ModDef::new("Top");
        let inst = top.instantiate(&leaf, Some("u_leaf"), None);

        assert!(inst.get_port("a").bit(0).has_physical_pin());
    }

    #[test]
    #[should_panic(expected = "has_physical_pin can only be called on single-bit port slices")]
    fn test_has_physical_pin_panics_on_multibit() {
        let module = ModDef::new("M");
        module.add_port("a", IO::Input(2));
        module.get_port("a").slice(1, 0).has_physical_pin();
    }

    #[test]
    fn test_place_across_connectivity_driven() {
        let module = ModDef::new("Top");
        module.add_port("x_in", IO::Input(2));
        module.add_port("x_out", IO::Output(2));

        let leaf = ModDef::new("Leaf");
        leaf.add_port("y_in", IO::Input(1));

        let leaf_inst = module.instantiate(&leaf, Some("leaf_inst"), None);
        leaf_inst
            .get_port("y_in")
            .connect(&module.get_port("x_in").bit(0));

        module.set_width_height(10, 10);
        let pin_shape = Polygon::from_width_height(1, 1);
        let mut tracks = TrackDefinitions::default();
        tracks.add_track(TrackDefinition::new(
            "M1",
            0,
            1,
            TrackOrientation::Vertical,
            Some(pin_shape.clone()),
            None,
        ));
        module.set_track_definitions(tracks);

        let x_out_bit_0 =
            PhysicalPin::from_translation("M1", pin_shape.clone(), Coordinate { x: 2, y: 0 });
        let x_out_bit_1 =
            PhysicalPin::from_translation("M1", pin_shape.clone(), Coordinate { x: 5, y: 0 });
        module.place_pin("x_out", 0, x_out_bit_0);
        module.place_pin("x_out", 1, x_out_bit_1);

        module.get_port("x_in").connect(&module.get_port("x_out"));
        module
            .get_port("x_in")
            .bit(0)
            .connect(&leaf_inst.get_port("y_in"));

        module.get_port("x_in").slice(1, 0).place_across();

        let x_in_bit_0 = module.get_port("x_in").bit(0).get_physical_pin();
        let x_in_bit_1 = module.get_port("x_in").bit(1).get_physical_pin();
        assert_eq!(x_in_bit_0.translation(), Coordinate { x: 2, y: 10 });
        assert_eq!(x_in_bit_1.translation(), Coordinate { x: 5, y: 10 });
    }
}
