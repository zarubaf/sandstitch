// SPDX-License-Identifier: Apache-2.0

use crate::{ConvertibleToPortSlice, ConvertibleToPortSliceVec, ModInst, PipelineConfig, Port};

impl Port {
    /// Connects this port to a net with a specific name.
    #[track_caller]
    pub fn specify_net_name(&self, net: &str) {
        self.to_port_slice().specify_net_name(net);
    }

    /// Connects this port to another port or port slice. See
    /// [`PortSlice::connect`] for the checks performed.
    #[track_caller]
    pub fn connect<T: ConvertibleToPortSlice>(&self, other: &T) {
        self.to_port_slice().connect(other);
    }

    /// Connects this port to `other` as raw bits; only widths must match.
    /// See [`PortSlice::connect_bits`].
    #[track_caller]
    pub fn connect_bits(&self, other: &impl ConvertibleToPortSlice) {
        self.to_port_slice().connect_bits(other);
    }

    #[track_caller]
    pub fn connect_pipeline<T: ConvertibleToPortSlice>(&self, other: &T, pipeline: PipelineConfig) {
        self.connect_generic(other, Some(pipeline));
    }

    /// Jam-connects this port and `other` LSB-first and marks any remainder on
    /// either side with `unused_or_tieoff(0)`.
    #[track_caller]
    pub fn todo_jam_connect<T: ConvertibleToPortSliceVec>(&self, other: &T) {
        self.to_port_slice().todo_jam_connect(other);
    }

    #[track_caller]
    pub(crate) fn connect_generic<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        pipeline: Option<PipelineConfig>,
    ) {
        self.to_port_slice().connect_generic(other, pipeline);
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this port to another port or port slice.
    #[track_caller]
    pub fn connect_through<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        through: &[&ModInst],
        prefix: impl AsRef<str>,
    ) {
        self.to_port_slice().connect_through(other, through, prefix);
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this port to another port or port slice, with
    /// optional pipelining for each connection.
    #[track_caller]
    pub fn connect_through_generic<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        through: &[(&ModInst, Option<PipelineConfig>)],
        prefix: impl AsRef<str>,
    ) {
        self.to_port_slice()
            .connect_through_generic(other, through, prefix);
    }
}
