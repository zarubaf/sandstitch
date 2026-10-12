// SPDX-License-Identifier: Apache-2.0

//! Physical-design methods of [`ModDef`]: shape, layer, keepout, edges and
//! routing tracks.

use crate::physical::geometry::{BoundingBox, Coordinate, Edge, EdgeOrientation, Polygon};
use crate::physical::tracks::{
    TrackDefinition, TrackDefinitions, TrackOccupancies, TrackOccupancy,
};
use crate::{ModDef, for_each_edge_direction};
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLockReadGuard, RwLockWriteGuard,
};

macro_rules! define_keepout_on_named_edge {
    ($edge_name:ident, $const_name:path) => {
        paste::paste! {
            #[doc = concat!(
                "Marks the specified tracks on the ",
                stringify!($edge_name),
                " edge as a keepout region using the provided polygon."
            )]
            pub fn [<define_keepout_on_ $edge_name _edge>](
                &self,
                layer: impl AsRef<str>,
                track_index: usize,
                polygon: &Polygon,
            ) {
                self.define_keepout_on_edge_index($const_name, layer, track_index, polygon);
            }
        }
    };
}

impl ModDef {
    /// Define a rectangular shape at (0, 0) with width and height. This is
    /// shorthand for set_shape with four rectilinear points.
    pub fn set_width_height(&self, width: i64, height: i64) {
        assert!(width > 0 && height > 0, "Width and height must be positive");
        self.set_shape(Polygon::from_width_height(width, height));
    }

    /// Define a rectilinear polygonal outline by its vertices in order
    pub fn set_shape(&self, shape: Polygon) {
        assert!(
            shape.is_rectilinear(),
            "A ModDef shape must be rectilinear."
        );
        assert!(
            shape.is_clockwise(),
            "ModDef shape edges must be defined in a clockwise order."
        );
        assert!(
            shape.starts_with_leftmost_vertical_edge(),
            "ModDef shapes must start with the leftmost vertical edge."
        );
        let mut core = self.core.write();
        core.track_occupancies = Some(TrackOccupancies::new(shape.num_edges()));
        core.shape = Some(shape);
    }

    /// Define the layer of this module.
    pub fn set_layer(&self, layer: impl AsRef<str>) {
        let mut core = self.core.write();
        core.layer = Some(layer.as_ref().to_string());
    }

    /// Returns this module's shape and its layer, if defined.
    pub fn get_shape(&self) -> Option<Polygon> {
        self.core.read().shape.clone()
    }

    /// Defines a polygon to use instead of the module shape when checking for
    /// overlaps with other placed instances.
    pub fn set_keepout(&self, keepout: Polygon) {
        self.core.write().keepout = Some(keepout);
    }

    /// Defines a rectangular instance keepout by expanding this module's
    /// bounding box by `x_margin` horizontally and `y_margin` vertically.
    pub fn set_keepout_from_margins(&self, x_margin: i64, y_margin: i64) {
        assert!(
            x_margin >= 0 && y_margin >= 0,
            "Keepout margins must be non-negative"
        );
        let bbox = self
            .bbox()
            .unwrap_or_else(|| panic!("Cannot define a keepout for a ModDef with no bounding box"));
        self.set_keepout(Polygon::from_bbox(&BoundingBox {
            min_x: bbox.min_x - x_margin,
            max_x: bbox.max_x + x_margin,
            min_y: bbox.min_y - y_margin,
            max_y: bbox.max_y + y_margin,
        }));
    }

    /// Returns the instance-overlap keepout, if one is defined.
    pub fn get_keepout(&self) -> Option<Polygon> {
        self.core.read().keepout.clone()
    }

    /// Removes the instance-overlap keepout.
    pub fn clear_keepout(&self) {
        self.core.write().keepout = None;
    }

    /// Returns this module's layer, if defined.
    pub fn get_layer(&self) -> Option<String> {
        self.core.read().layer.clone()
    }

    /// Returns the number of edges (vertices) of the current shape, if set.
    pub fn get_num_edges(&self) -> usize {
        self.core
            .read()
            .shape
            .as_ref()
            .map(|s| s.num_edges())
            .unwrap_or(0)
    }

    /// Sets the track definitions for this module.
    pub fn set_track_definitions(&self, track_definitions: TrackDefinitions) {
        let mut core = self.core.write();
        let shape = core
            .shape
            .as_ref()
            .expect("Shape must be set before setting track definitions")
            .clone();
        core.track_definitions = Some(track_definitions);

        // For each edge, for each track definition (layer), initialize occupancy
        let track_defs = core.track_definitions.as_ref().unwrap().clone();
        let occupancies = core
            .track_occupancies
            .as_mut()
            .expect("Track occupancies must be initialized before setting track definitions");

        for (edge_index, edge_map) in occupancies.0.iter_mut().enumerate() {
            let edge = shape.get_edge(edge_index);
            for (layer_name, track_def) in track_defs.0.iter() {
                if let Some(range) = edge.get_index_range(track_def) {
                    let length = (range.max.unwrap() - range.min.unwrap() + 1) as usize;
                    edge_map.insert(layer_name.clone(), TrackOccupancy::new(length));
                }
            }
        }
    }

    /// Returns a shared reference to this module's track definitions, if set.
    pub fn get_track_definitions(&self) -> Option<MappedRwLockReadGuard<'_, TrackDefinitions>> {
        RwLockReadGuard::try_map(self.core.read(), |core| core.track_definitions.as_ref()).ok()
    }

    /// Returns a mutable reference to this module's track definitions, if set.
    pub fn get_track_definitions_mut(
        &self,
    ) -> Option<MappedRwLockWriteGuard<'_, TrackDefinitions>> {
        RwLockWriteGuard::try_map(self.core.write(), |core| core.track_definitions.as_mut()).ok()
    }

    /// Looks up the [`TrackDefinition`] for `name`, if one has been registered.
    pub fn get_track(&self, name: impl AsRef<str>) -> Option<TrackDefinition> {
        let core_borrowed = self.core.read();
        let track_definitions = &core_borrowed.track_definitions;
        track_definitions
            .as_ref()
            .and_then(|t| t.get_track(name.as_ref()).cloned())
    }

    /// Returns the polygon edge at `edge_index`, or `None` if the shape is not
    /// defined or the index is out of bounds.
    pub fn get_edge(&self, edge_index: usize) -> Option<Edge> {
        let core_borrowed = self.core.read();
        let shape = &core_borrowed.shape;
        shape.as_ref().map(|s| s.get_edge(edge_index))
    }

    /// Returns the nearest usable track index relative to the start of
    /// `edge_index` for `coordinate`, if the layer and edge are compatible.
    pub fn nearest_relative_track_index(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
        coordinate: &Coordinate,
    ) -> Option<usize> {
        let layer = layer.as_ref();
        let shape = self.get_shape()?;
        let track = self.get_track(layer)?;
        let edge = shape.get_edge(edge_index);
        let orientation = edge.orientation()?;
        let track_range = edge.get_index_range(&track)?;
        let min_index = track_range.min?;
        let max_index = track_range.max?;

        let axis_coordinate = match orientation {
            EdgeOrientation::North | EdgeOrientation::South => coordinate.y,
            EdgeOrientation::East | EdgeOrientation::West => coordinate.x,
        };

        let absolute_track_index = track.nearest_track_index(axis_coordinate);
        if (min_index <= absolute_track_index) && (absolute_track_index <= max_index) {
            Some((absolute_track_index - min_index) as usize)
        } else {
            None
        }
    }

    /// Marks the inclusive track index range as occupied by an existing pin.
    pub fn mark_pin_range(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
        min_index: i64,
        max_index: i64,
    ) {
        let mut core = self.core.write();
        let occupancies = core
            .track_occupancies
            .as_mut()
            .expect("Track occupancies not initialized");
        if let Some(occupancy) = occupancies.get_occupancy_mut(edge_index, layer.as_ref()) {
            occupancy.mark_pin(min_index, max_index);
        }
    }

    /// Marks the inclusive track index range as blocked by a keepout region.
    pub fn mark_keepout_range(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
        min_index: i64,
        max_index: i64,
    ) {
        let mut core = self.core.write();
        let occupancies = core
            .track_occupancies
            .as_mut()
            .expect("Track occupancies not initialized");
        if let Some(occupancy) = occupancies.get_occupancy_mut(edge_index, layer.as_ref()) {
            occupancy.mark_keepout(min_index, max_index);
        }
    }

    /// Records both the pin and the keepout envelopes for a placed pin in a
    /// single call.
    pub fn mark_pin_and_keepout_ranges(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
        pin_min_index: i64,
        pin_max_index: i64,
        keepout_min_index: i64,
        keepout_max_index: i64,
    ) {
        let mut core = self.core.write();
        let occupancies = core
            .track_occupancies
            .as_mut()
            .expect("Track occupancies not initialized");
        if let Some(occupancy) = occupancies.get_occupancy_mut(edge_index, layer.as_ref()) {
            occupancy.place_pin_and_keepout(
                pin_min_index,
                pin_max_index,
                keepout_min_index,
                keepout_max_index,
            );
        }
    }

    for_each_edge_direction!(define_keepout_on_named_edge);

    /// Marks the keepout polygon corresponding to `track_index` on
    /// `edge_index`, using the provided shape to derive track coverage.
    pub fn define_keepout_on_edge_index(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
        track_index: usize,
        polygon: &Polygon,
    ) {
        let layer_ref = layer.as_ref();

        let transform = self.track_index_to_transform(edge_index, layer_ref, track_index);
        let polygon = polygon.apply_transform(&transform);

        let (keepout_min_track, keepout_max_track) =
            self.track_range_for_polygon(edge_index, layer_ref, &polygon);

        self.mark_keepout_range(edge_index, layer_ref, keepout_min_track, keepout_max_track);
    }

    /// Returns the ordered list of routing layer names with defined track
    /// families.
    pub fn get_layers(&self) -> Vec<String> {
        self.core
            .read()
            .track_definitions
            .as_ref()
            .map(|td| td.0.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Fetches a clone of the occupancy bitmap for internal placement checks.
    pub(crate) fn get_occupancy(
        &self,
        edge_index: usize,
        layer: impl AsRef<str>,
    ) -> Option<TrackOccupancy> {
        self.core
            .read()
            .track_occupancies
            .as_ref()
            .and_then(|occupancies| {
                occupancies
                    .get_occupancy(edge_index, layer.as_ref())
                    .cloned()
            })
    }

    pub fn set_default_connection_max_distance(&self, value: Option<i64>) {
        self.core.write().default_connection_max_distance = value;
    }

    pub fn get_default_connection_max_distance(&self) -> Option<i64> {
        self.core.read().default_connection_max_distance
    }
}
