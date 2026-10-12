// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use crate::diagnostic::codes;
use crate::model::intf::members::{describe, direction, pairs_with};
use crate::{
    ConvertibleToPortSliceVec, Diagnostic, Intf, Label, ModInst, PipelineConfig, PortSlice,
    Severity, Span,
};

impl Intf {
    /// Connects this interface to another interface. Interfaces are connected
    /// by matching up ports with the same function name and connecting them.
    /// For example, if this interface is {"data": "a_data", "valid": "a_valid"}
    /// and the other interface is {"data": "b_data", "valid": "b_valid"}, then
    /// "a_data" will be connected to "b_data" and "a_valid" will be connected
    /// to "b_valid".
    ///
    /// A name present in both directions (see [`ModDef::def_intf_members`])
    /// pairs each member with the same-named member driven the other way:
    /// an output with an input between instances, ports of the same
    /// direction between a module and its instance.
    ///
    /// Unless `allow_mismatch` is `true`, a member without a partner on the
    /// other side is reported (S0207). Continuing the previous example, if
    /// this interface also contained function "ready", but the other
    /// interface did not, this method would panic unless `allow_mismatch` was
    /// `true`.
    ///
    /// [`ModDef::def_intf_members`]: crate::ModDef::def_intf_members
    #[track_caller]
    pub fn connect(&self, other: &Intf, allow_mismatch: bool) {
        self.connect_generic(other, None, allow_mismatch, None::<&[&str]>);
    }

    /// Jam-connects this interface and `other` LSB-first over the ordered
    /// `PortSlice` lists and marks any remainder on either side with
    /// `unused_or_tieoff(0)`.
    pub fn todo_jam_connect<T: ConvertibleToPortSliceVec>(&self, other: &T) {
        let left = self.to_port_slice_vec();
        let right = other.to_port_slice_vec();
        PortSlice::todo_jam_connect_port_slices(&left, &right);
    }

    /// Connects this interface to another interface, skipping the specified functions.
    #[track_caller]
    pub fn connect_except<'a, I, T>(&self, other: &Intf, skip: Option<I>)
    where
        I: IntoIterator<Item = &'a T>,
        T: AsRef<str> + 'a,
    {
        self.connect_generic(other, None, false, skip);
    }

    /// Panics with S0209 if either interface has a name in both directions.
    #[track_caller]
    pub fn connect_pipeline(&self, other: &Intf, pipeline: PipelineConfig, allow_mismatch: bool) {
        self.connect_generic(other, Some(pipeline), allow_mismatch, None::<&[&str]>);
    }

    #[track_caller]
    pub fn connect_pipeline_except<'a, I, T>(
        &self,
        other: &Intf,
        pipeline: PipelineConfig,
        skip: Option<I>,
    ) where
        I: IntoIterator<Item = &'a T>,
        T: AsRef<str> + 'a,
    {
        self.connect_generic(other, Some(pipeline), false, skip);
    }

    #[track_caller]
    pub(crate) fn connect_generic<'a, I, T>(
        &self,
        other: &Intf,
        pipeline: Option<PipelineConfig>,
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

        if pipeline.is_some() {
            // Pipelines pair members by name only.
            self.get_port_slices();
            other.get_port_slices();
        }

        let ours = self.members();
        let theirs = other.members();
        let mut paired = vec![false; theirs.len()];
        let by_name = unique_names(&ours) && unique_names(&theirs);

        for (name, slice) in &ours {
            if is_skipped(name) {
                continue;
            }
            match partner(slice, name, &theirs, by_name) {
                Some(j) => {
                    paired[j] = true;
                    slice.connect_generic(&theirs[j].1, pipeline.clone());
                }
                None if allow_mismatch => {}
                None => panic_s0207(self, name, slice, other),
            }
        }

        if !allow_mismatch {
            for (j, (name, slice)) in theirs.iter().enumerate() {
                if !paired[j] && !is_skipped(name) {
                    panic_s0207(other, name, slice, self);
                }
            }
        }
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this interface to another interface.
    #[track_caller]
    pub fn connect_through(
        &self,
        other: &Intf,
        through: &[&ModInst],
        prefix: impl AsRef<str>,
        allow_mismatch: bool,
    ) {
        let mut through_generic = Vec::new();
        for inst in through {
            through_generic.push((*inst, None));
        }
        self.connect_through_generic(other, &through_generic, prefix, allow_mismatch);
    }

    /// Punches a sequence of feedthroughs through the specified module
    /// instances to connect this interface to another interface, with
    /// optional pipelining for each connection.
    #[track_caller]
    pub fn connect_through_generic(
        &self,
        other: &Intf,
        through: &[(&ModInst, Option<PipelineConfig>)],
        prefix: impl AsRef<str>,
        allow_mismatch: bool,
    ) {
        if through.is_empty() {
            self.connect(other, allow_mismatch);
            return;
        }

        // The ends are connected through feedthroughs only; record each
        // end-to-end pair so that their types are still checked.
        let location = std::panic::Location::caller();
        let other_ports = other.get_port_slices();
        for (func_name, self_port) in self.get_port_slices() {
            if let Some(other_port) = other_ports.get(&func_name) {
                self_port.store_record(other_port, location, false);
            }
        }

        let flipped = format!("{}_flipped_{}", prefix.as_ref(), self.get_intf_name());
        let original = format!("{}_original_{}", prefix.as_ref(), self.get_intf_name());

        for (i, (inst, pipeline)) in through.iter().enumerate() {
            self.feedthrough_generic(
                &inst.get_mod_def(),
                &flipped,
                &original,
                pipeline.as_ref().cloned(),
            );
            if i == 0 {
                self.connect(&inst.get_intf(&flipped), false);
            } else {
                through[i - 1]
                    .0
                    .get_intf(&original)
                    .connect(&inst.get_intf(&flipped), false);
            }

            if i == through.len() - 1 {
                other.connect(&inst.get_intf(&original), allow_mismatch);
            }
        }
    }
}

/// The index in `theirs` of the partner of member `name` (`slice`): the
/// same-named member driven the other way. Between interfaces whose names
/// are all unique (`by_name`), the same-named member is the partner
/// whatever its direction, as upstream, so that connecting it reports a
/// direction error.
fn partner(
    slice: &PortSlice,
    name: &str,
    theirs: &[(String, PortSlice)],
    by_name: bool,
) -> Option<usize> {
    theirs
        .iter()
        .position(|(n, s)| n == name && (by_name || pairs_with(slice, s)))
}

/// Whether every member name of `members` is unique.
fn unique_names(members: &[(String, PortSlice)]) -> bool {
    members
        .iter()
        .enumerate()
        .all(|(i, (name, _))| members[..i].iter().all(|(n, _)| n != name))
}

#[track_caller]
fn panic_s0207(intf: &Intf, name: &str, slice: &PortSlice, other: &Intf) -> ! {
    Diagnostic {
        code: codes::S0207,
        severity: Severity::Error,
        message: format!(
            "member `{name}` of `{}` has no partner in `{}`",
            intf.debug_string(),
            other.debug_string()
        ),
        primary: Label {
            span: Span::caller(),
            text: "interfaces connected here".into(),
        },
        secondary: vec![],
        notes: vec![format!(
            "`{}` ({}) needs a member `{name}` driven the other way",
            describe(slice),
            direction(slice)
        )],
        helps: vec!["pass `allow_mismatch = true` to leave unpaired members unconnected".into()],
    }
    .panic()
}
