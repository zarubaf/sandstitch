// SPDX-License-Identifier: Apache-2.0

use crate::PortKind;
use crate::model::connection::port_slice::{CallSite, ConnectionRecord};
use crate::model::port::PortDirectionality;
use crate::{
    ConvertibleToPortSlice, ConvertibleToPortSliceVec, IO, ModInst, PipelineConfig, Port,
    PortSlice, Span,
};
use std::sync::Arc;

impl PortSlice {
    /// Specifies the net name to be used for this port slice.
    #[track_caller]
    pub fn specify_net_name(&self, net: &str) {
        if let Port {
            kind: PortKind::ModInst { .. },
        } = &self.port
        {
            // Record and enforce uniqueness of explicitly specified net names
            // within the containing ModDef.
            {
                let core_rc = self.get_mod_def_core();
                let mut core = core_rc.write();
                if !core.specified_net_names.insert(net.to_string()) {
                    panic!(
                        "Net \"{}\" has already been manually specified in module {}.",
                        net, core.name
                    );
                };
            }

            let this = self.to_port_slice();
            let width = this.port.io().width();
            let other = crate::model::connection::connected_item::Wire {
                name: net.to_string(),
                width,
                msb: self.msb,
                lsb: self.lsb,
            };
            self.port
                .get_port_connections_define_if_missing()
                .write()
                .add_user(this, other, CallSite::here(), true);
        } else {
            panic!(
                "{} only works on ports (or slices of ports) on module instances",
                stringify!(specify_net_name)
            );
        }
    }

    /// Connects this port slice to another port or port slice. Performs some
    /// upfront checks to make sure that the connection is valid in terms of
    /// width and directionality. Panics if any of these checks fail.
    ///
    /// If both ends are typed (whole typed ports, fields or elements), their
    /// types must be equivalent; [`ModDef::check`](crate::ModDef::check)
    /// reports a violation (S0201) at this call site.
    #[track_caller]
    pub fn connect<T: ConvertibleToPortSlice>(&self, other: &T) {
        self.connect_generic(other, None);
    }

    /// Connects this port slice to `other` as raw bits: only widths must
    /// match, whatever the types of the ends. Use it to reinterpret bits
    /// deliberately; the connection is recorded as such.
    #[track_caller]
    pub fn connect_bits(&self, other: &impl ConvertibleToPortSlice) {
        self.connect_with(other, None, true, CallSite::here());
    }

    #[track_caller]
    pub fn connect_pipeline<T: ConvertibleToPortSlice>(&self, other: &T, pipeline: PipelineConfig) {
        self.connect_generic(other, Some(pipeline));
    }

    /// Connects this `PortSlice` to `other`, consuming both sides from LSB to
    /// MSB as an ordered stream of bits. Any unconsumed remainder on either
    /// side is marked with `unused_or_tieoff(0)`.
    #[track_caller]
    pub fn todo_jam_connect<T: ConvertibleToPortSliceVec>(&self, other: &T) {
        let left = self.to_port_slice_vec();
        let right = other.to_port_slice_vec();
        Self::todo_jam_connect_port_slices(&left, &right);
    }

    #[track_caller]
    pub(crate) fn todo_jam_connect_port_slices(left: &[PortSlice], right: &[PortSlice]) {
        let mut left_idx = 0usize;
        let mut right_idx = 0usize;
        let mut left_lsb = 0usize;
        let mut right_lsb = 0usize;

        while left_idx < left.len() && right_idx < right.len() {
            let left_width = left[left_idx].width();
            let right_width = right[right_idx].width();

            if left_lsb == left_width {
                left_idx += 1;
                left_lsb = 0;
                continue;
            }

            if right_lsb == right_width {
                right_idx += 1;
                right_lsb = 0;
                continue;
            }

            let chunk_width = usize::min(left_width - left_lsb, right_width - right_lsb);

            left[left_idx]
                .slice_with_offset_and_width(left_lsb, chunk_width)
                // A jam connection streams bits, whatever the types.
                .connect_bits(
                    &right[right_idx].slice_with_offset_and_width(right_lsb, chunk_width),
                );

            left_lsb += chunk_width;
            right_lsb += chunk_width;

            if left_lsb == left_width {
                left_idx += 1;
                left_lsb = 0;
            }

            if right_lsb == right_width {
                right_idx += 1;
                right_lsb = 0;
            }
        }

        mark_jam_remainder(left, left_idx, left_lsb);
        mark_jam_remainder(right, right_idx, right_lsb);
    }

    #[track_caller]
    pub(crate) fn connect_generic<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        pipeline: Option<PipelineConfig>,
    ) {
        self.connect_with(other, pipeline, false, CallSite::here());
    }

    fn connect_with<T: ConvertibleToPortSlice + ?Sized>(
        &self,
        other: &T,
        pipeline: Option<PipelineConfig>,
        bits: bool,
        site: CallSite,
    ) {
        let other_as_slice = other.to_port_slice();

        if !Arc::ptr_eq(&self.get_mod_def_core(), &other_as_slice.get_mod_def_core()) {
            panic!(
                "Cannot connect {} and {} because they are in different module definitions",
                self.debug_string(),
                other_as_slice.debug_string()
            );
        }

        if self.width() != other_as_slice.width() {
            panic!(
                "Width mismatch when connecting {} and {}",
                self.debug_string(),
                other_as_slice.debug_string()
            );
        }

        let self_directionality = self.get_directionality();
        let other_directionality = other_as_slice.get_directionality();

        let generate_incompatibility_mesage = || {
            format!(
                "Cannot connect {} and {} because they have incompatible directions.",
                self.debug_string(),
                other_as_slice.debug_string()
            )
        };

        if !self_directionality.compatible_with(&other_directionality) {
            panic!("{}", generate_incompatibility_mesage());
        }

        if let Some(pipeline) = pipeline {
            let repeater_mod_def = pipeline.to_mod_def(self.width());

            let mod_def = self.get_mod_def();
            let repeater_inst_name = mod_def.resolve_pipeline_instance_name(&pipeline);

            // Figure out which port is the driver and which is the receiver, otherwise
            // panic. Note that InOut ports cannot be pipelined.
            let (driver, receiver) = match (self_directionality, other_directionality) {
                (PortDirectionality::Driver, PortDirectionality::Receiver) => {
                    (self, &other_as_slice)
                }
                (PortDirectionality::Receiver, PortDirectionality::Driver) => {
                    (&other_as_slice, self)
                }
                (PortDirectionality::InOut, _) => {
                    panic!("Cannot pipeline InOut port {}", self.debug_string())
                }
                (_, PortDirectionality::InOut) => panic!(
                    "Cannot pipeline InOut port {}",
                    other_as_slice.debug_string()
                ),
                _ => {
                    // This should be unreachable due to the previous compatible_with() check, but
                    // we keep descriptive error message to make debugging easier if it happens.
                    panic!("{}", generate_incompatibility_mesage());
                }
            };

            let repeater_instance =
                mod_def.instantiate(&repeater_mod_def, Some(&repeater_inst_name), None);
            let mod_def_clk = if mod_def.has_port(&pipeline.clk) {
                mod_def.get_port(&pipeline.clk)
            } else {
                mod_def.add_port(&pipeline.clk, IO::Input(1))
            };
            let user_connect = |port: Port, other: &PortSlice| {
                port.to_port_slice()
                    .connect_with(other, None, false, site.clone());
            };
            user_connect(
                repeater_instance.get_port("clk"),
                &mod_def_clk.to_port_slice(),
            );
            user_connect(repeater_instance.get_port("in"), driver);
            user_connect(repeater_instance.get_port("out"), receiver);
            repeater_instance
                .get_port("out_stages")
                .to_port_slice()
                .unused_at(site.clone());
            // The ends are not connected directly, but their types must
            // still agree.
            self.store_record(&other_as_slice, site.span.clone(), bits);
        } else {
            // `a.connect(&b)` followed by `b.connect(&a)` is one connection:
            // the other end already holds its record.
            let record = self
                .typed_record(&other_as_slice, site.span.clone(), bits)
                .filter(|record| {
                    other_as_slice
                        .port
                        .get_port_connections()
                        .is_none_or(|c| !c.read().has_swapped_record(record))
                });
            {
                let connections = self.port.get_port_connections_define_if_missing();
                let mut connections = connections.write();
                connections.add_user(self.clone(), other_as_slice.clone(), site.clone(), true);
                if let Some(record) = record {
                    connections.add_record(record);
                }
            }
            other_as_slice
                .port
                .get_port_connections_define_if_missing()
                .write()
                .add_user(other_as_slice.clone(), self.clone(), site, false);
        }
    }

    /// A record of a connection from `self` to `other`, if both ends are
    /// typed: only those are compared by `check` and named by type.
    fn typed_record(&self, other: &PortSlice, span: Span, bits: bool) -> Option<ConnectionRecord> {
        (self.port_type().is_some() && other.port_type().is_some()).then(|| ConnectionRecord {
            this: self.clone(),
            other: other.clone(),
            span,
            bits,
        })
    }

    /// Records a connection between typed ends that are connected
    /// indirectly (through a pipeline or feedthroughs).
    pub(crate) fn store_record(&self, other: &PortSlice, span: Span, bits: bool) {
        if let Some(record) = self.typed_record(other, span, bits) {
            self.port
                .get_port_connections_define_if_missing()
                .write()
                .add_record(record);
        }
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this port slice to another port or port slice.
    #[track_caller]
    pub fn connect_through<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        through: &[&ModInst],
        prefix: impl AsRef<str>,
    ) {
        let mut through_generic = Vec::new();
        for inst in through {
            through_generic.push((*inst, None));
        }
        self.connect_through_generic(other, &through_generic, prefix);
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this port slice to another port or port slice,
    /// with optional pipelining for each connection.
    #[track_caller]
    pub fn connect_through_generic<T: ConvertibleToPortSlice>(
        &self,
        other: &T,
        through: &[(&ModInst, Option<PipelineConfig>)],
        prefix: impl AsRef<str>,
    ) {
        if through.is_empty() {
            self.connect(other);
            return;
        }

        self.store_record(&other.to_port_slice(), Span::caller(), false);

        let flipped = format!("{}_flipped", prefix.as_ref());
        let original = format!("{}_original", prefix.as_ref());

        for (i, (inst, pipeline)) in through.iter().enumerate() {
            let (flipped_port, original_port) = self.feedthrough_generic(
                &inst.get_mod_def(),
                &flipped,
                &original,
                pipeline.as_ref().cloned(),
            );

            // These are ModDef ports, so we need to assign them to the specific
            // instance in order to wire them up.
            let flipped_port = flipped_port.assign_to_inst(inst);
            let original_port = original_port.assign_to_inst(inst);

            if i == 0 {
                self.connect(&flipped_port);
            } else {
                through[i - 1].0.get_port(&original).connect(&flipped_port);
            }

            if i == through.len() - 1 {
                other.to_port_slice().connect(&original_port);
            }
        }
    }
}

#[track_caller]
fn mark_jam_remainder(port_slices: &[PortSlice], mut idx: usize, lsb: usize) {
    if idx >= port_slices.len() {
        return;
    }

    if lsb > 0 {
        let width = port_slices[idx].width();
        port_slices[idx]
            .slice_with_offset_and_width(lsb, width - lsb)
            .unused_or_tieoff(0);
        idx += 1;
    }

    for port_slice in port_slices.iter().skip(idx) {
        port_slice.unused_or_tieoff(0);
    }
}
