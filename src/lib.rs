// SPDX-License-Identifier: Apache-2.0

//! Sandstitch (crate `topstitch`): build SystemVerilog designs from Rust.
//!
//! The source is organised by concern:
//!
//! - `model`: the structural model (`ModDef`, `ModInst`, `Port`, `PortType`,
//!   `PortSlice`, `Intf`, connections, parameters).
//! - `import`: SystemVerilog import through Slang (the only user of
//!   `slang_rs`), parameterization and package extraction.
//! - `emit`: SystemVerilog emission.
//! - `check`: structural validation.
//! - `resolve`: connection resolution, what drives each bit of each end of a
//!   definition (crate-internal).
//! - `physical`: geometry, tracks, pins, placement, LEF/DEF, and the physical
//!   methods of the model types.
//! - [`diagnostic`]: structured diagnostics.
//!
//! The public API is re-exported at the crate root; its vocabulary is
//! described in `doc/product.md`, "Generator and API contract".

mod check;
pub mod diagnostic;
mod emit;
mod import;
mod model;
mod physical;
mod resolve;
mod util;

use model::intf::IntfKind;
use model::mod_def::ModDefCore;
use model::port::PortKind;

// Structural model.
pub use model::funnel::Funnel;
pub use model::intf::Intf;
pub use model::io::IO;
pub use model::metadata::{Metadata, MetadataKey, MetadataValue};
pub use model::mod_def::{ConvertibleToModDef, ModDef};
pub use model::mod_inst::ModInst;
pub use model::package::{PackageDef, PackageType, PackageValue};
pub use model::parameter::{ParameterType, ParameterValue};
pub use model::pipeline::PipelineConfig;
pub use model::port::Port;
pub use model::port_slice::{ConvertibleToPortSlice, ConvertibleToPortSliceVec, PortSlice, Select};
pub use model::port_type::{Dim, PortField, PortType, QualifiedName, TypeDifference};
pub use model::usage::Usage;

// Import.
pub use import::package::{
    Package, Parameter, extract_packages_from_verilog, extract_packages_from_verilog_file,
    extract_packages_from_verilog_files, extract_packages_with_config,
};
pub use import::parser_cfg::ParserConfig;
pub use import::slang_invocations;
pub use import::specialize::SpecializationKey;

// Emission.
pub use emit::manifest::Manifest;
pub use emit::verilog::EmitOptions;

// Physical design.
pub use physical::edges::{
    BOTTOM_EDGE_INDEX, EAST_EDGE_INDEX, LEFT_EDGE_INDEX, NORTH_EDGE_INDEX, RIGHT_EDGE_INDEX,
    SOUTH_EDGE_INDEX, TOP_EDGE_INDEX, WEST_EDGE_INDEX,
};
pub use physical::geometry::{
    BoundingBox, Coordinate, EdgeOrientation, Mat3, Orientation, PhysicalPin, Placement, Polygon,
    Range,
};
pub use physical::lefdef;
pub use physical::lefdef::{LefDefOptions, MultiplePinShapesPolicy};
pub use physical::pins::SpreadPinsOptions;
pub use physical::placement::CalculatedPlacement;
pub use physical::tracks::{
    PinPlacementError, TrackDefinition, TrackDefinitions, TrackOrientation,
};

// Diagnostics.
pub use diagnostic::{
    Code, Diagnostic, Diagnostics, ImportError, Label, ObjectRef, Severity, Span,
};
