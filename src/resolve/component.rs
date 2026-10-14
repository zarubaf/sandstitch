// SPDX-License-Identifier: Apache-2.0

//! Connected components of a definition's bits, their drivers, and the
//! connection diagnostics (S0401 to S0406, S0409).
//!
//! Every stored connection, tieoff, unused mark and net name covers a range
//! of the end it is stored on. Each end is cut into atoms: the covered ranges
//! between breakpoints, where the breakpoints are the boundaries of the
//! stored ranges, carried across port-to-port connections until both sides
//! of every connection are cut alike. Atoms joined by connections form the
//! components. The pass works on atoms, never on single bits.

use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::diagnostic::codes;
use crate::model::connection::connected_item::{ConnectedItem, Tieoff};
use crate::model::connection::port_slice::{CallSite, PortSliceConnection, PortSliceConnections};
use crate::{Code, Diagnostic, IO, Label, ModDefCore, ObjectRef, Port, PortSlice, Severity, Span};

/// What drives a component.
#[derive(Debug)]
#[cfg_attr(not(test), expect(dead_code, reason = "read by the snapshot (#45)"))]
pub(crate) enum Driver {
    DefInput(PortSlice),
    InstOutput(PortSlice),
    Constant(Tieoff),
    Bidirectional(Vec<PortSlice>),
}

/// Bits of a definition joined by connections.
#[derive(Debug)]
#[cfg_attr(not(test), expect(dead_code, reason = "read by the snapshot (#45)"))]
pub(crate) struct Component {
    /// End ranges, in declaration order of their ends.
    pub(crate) members: Vec<PortSlice>,
    /// `None` when undriven or in error.
    pub(crate) driver: Option<Driver>,
}

/// The direction of an end as seen inside the definition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    DefInput,
    DefOutput,
    DefInOut,
    InstInput,
    InstOutput,
    InstInOut,
}

impl Kind {
    fn of(is_def: bool, io: &IO) -> Kind {
        match (is_def, io) {
            (true, IO::Input(_)) => Kind::DefInput,
            (true, IO::Output(_)) => Kind::DefOutput,
            (true, IO::InOut(_)) => Kind::DefInOut,
            (false, IO::Input(_)) => Kind::InstInput,
            (false, IO::Output(_)) => Kind::InstOutput,
            (false, IO::InOut(_)) => Kind::InstInOut,
        }
    }

    /// A definition input or an instance output: the end drives its bits.
    fn drives(self) -> bool {
        matches!(self, Kind::DefInput | Kind::InstOutput)
    }

    fn is_inout(self) -> bool {
        matches!(self, Kind::DefInOut | Kind::InstInOut)
    }

    fn may_be_tied_off(self) -> bool {
        !self.drives()
    }

    fn may_be_unused(self) -> bool {
        self.drives() || self.is_inout()
    }
}

/// A port of an instance or of the definition.
struct End {
    inst: Option<String>,
    port_name: String,
    width: usize,
    kind: Kind,
    /// The port, taken from a connection; `None` for an end without any.
    port: Option<Port>,
}

impl End {
    /// `inst.port` or `port`.
    fn name(&self) -> String {
        match &self.inst {
            Some(inst) => format!("{inst}.{}", self.port_name),
            None => self.port_name.clone(),
        }
    }
}

/// What a stored connection says about its range.
enum Intent {
    Connected { other: usize, other_lsb: usize },
    Tieoff(Tieoff),
    Unused,
    Wire,
}

/// A stored connection on the range `[msb:lsb]` of the end it is stored on.
struct Item {
    end: usize,
    msb: usize,
    lsb: usize,
    intent: Intent,
    site: CallSite,
    /// Whether the call was made on this end (`self` of `connect`).
    called_on: bool,
}

impl Item {
    fn label(&self) -> &'static str {
        match self.intent {
            Intent::Connected { .. } => "connected here",
            Intent::Tieoff(_) => "tied off here",
            Intent::Unused => "marked unused here",
            Intent::Wire => "net named here",
        }
    }

    /// The intent as S0404 names it.
    fn intent_name(&self) -> &'static str {
        match self.intent {
            Intent::Connected { .. } => "connected",
            Intent::Tieoff(_) => "tied off",
            Intent::Unused => "marked unused",
            Intent::Wire => "named a net",
        }
    }

    fn tieoff(&self) -> Option<&Tieoff> {
        match &self.intent {
            Intent::Tieoff(tieoff) => Some(tieoff),
            _ => None,
        }
    }

    fn is_unused(&self) -> bool {
        matches!(self.intent, Intent::Unused)
    }

    fn is_wire(&self) -> bool {
        matches!(self.intent, Intent::Wire)
    }
}

/// A covered range of one end that no breakpoint splits, with the items
/// covering it.
struct Atom {
    end: usize,
    msb: usize,
    lsb: usize,
    items: Vec<usize>,
}

/// The ranges `(lsb, msb)` between consecutive breakpoints of one end,
/// ascending, with the atom of each covered range.
#[derive(Default)]
struct Intervals {
    ranges: Vec<(usize, usize)>,
    atoms: Vec<Option<usize>>,
}

impl Intervals {
    /// The index of the range starting at breakpoint `lsb`.
    fn index(&self, lsb: usize) -> usize {
        self.ranges
            .binary_search_by_key(&lsb, |&(l, _)| l)
            .expect("ranges start at breakpoints")
    }

    /// The atoms of the ranges that make up `[msb:lsb]`, from `lsb` up.
    fn atoms_in(&self, msb: usize, lsb: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let first = self.index(lsb);
        self.ranges[first..]
            .iter()
            .zip(&self.atoms[first..])
            .take_while(move |((_, m), _)| *m <= msb)
            .map(|((l, _), atom)| (*l, atom.expect("stored ranges are covered")))
    }
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }

    /// Joins the sets of `a` and `b`; false if they were one set already.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return false;
        }
        self.0[a.max(b)] = a.min(b);
        true
    }
}

/// A definition's ends and the connections stored on each.
struct Collected {
    def: String,
    span: Span,
    ends: Vec<End>,
    stored: Vec<(usize, Vec<PortSliceConnection>)>,
}

/// Every end of `core` (instance ports, then definition ports, in
/// declaration order) and the connections stored on each, under one short
/// read lock.
fn collect(core: &Arc<RwLock<ModDefCore>>) -> Collected {
    let core = core.read();
    let mut ends = Vec::new();
    let mut stored = Vec::new();
    let mut add = |end: End, connections: Option<&Arc<RwLock<PortSliceConnections>>>| {
        if let Some(connections) = connections {
            stored.push((
                ends.len(),
                connections.read().into_iter().cloned().collect(),
            ));
        }
        ends.push(end);
    };
    for (inst_name, inst) in &core.instances {
        let connections = core.mod_inst_connections.get(inst_name);
        for (port_name, io) in &inst.read().ports {
            let end = End {
                inst: Some(inst_name.clone()),
                port_name: port_name.clone(),
                width: io.width(),
                kind: Kind::of(false, io),
                port: None,
            };
            add(end, connections.and_then(|c| c.get(port_name)));
        }
    }
    for (port_name, io) in &core.ports {
        let end = End {
            inst: None,
            port_name: port_name.clone(),
            width: io.width(),
            kind: Kind::of(true, io),
            port: None,
        };
        add(end, core.mod_def_connections.get(port_name));
    }
    Collected {
        def: core.name.clone(),
        span: core.span.clone(),
        ends,
        stored,
    }
}

/// The items of the stored connections, and the port-to-port connections
/// among them, each once (the copies stored on both ends are one edge).
fn items(
    ends: &mut [End],
    stored: Vec<(usize, Vec<PortSliceConnection>)>,
) -> (Vec<Item>, Vec<usize>) {
    let index: HashMap<(Option<String>, String), usize> = ends
        .iter()
        .enumerate()
        .map(|(i, e)| ((e.inst.clone(), e.port_name.clone()), i))
        .collect();
    let mut items = Vec::new();
    let mut edges = Vec::new();
    let mut seen = HashSet::new();
    for (end, connections) in stored {
        for c in connections {
            ends[end].port.get_or_insert_with(|| c.this.port.clone());
            let intent = match &c.other {
                ConnectedItem::PortSlice(other) => {
                    let key = (
                        other.port.inst_name().map(str::to_string),
                        other.port.name().to_string(),
                    );
                    let other_end = *index
                        .get(&key)
                        .expect("connected ends belong to the definition");
                    ends[other_end]
                        .port
                        .get_or_insert_with(|| other.port.clone());
                    let this = (end, c.this.msb, c.this.lsb);
                    let that = (other_end, other.msb, other.lsb);
                    if seen.insert((this.min(that), this.max(that))) {
                        edges.push(items.len());
                    }
                    Intent::Connected {
                        other: other_end,
                        other_lsb: other.lsb,
                    }
                }
                ConnectedItem::Tieoff(tieoff) => Intent::Tieoff(tieoff.clone()),
                ConnectedItem::Unused(_) => Intent::Unused,
                ConnectedItem::Wire(_) => Intent::Wire,
            };
            items.push(Item {
                end,
                msb: c.this.msb,
                lsb: c.this.lsb,
                intent,
                site: c.site,
                called_on: c.called_on,
            });
        }
    }
    (items, edges)
}

/// Cuts every end into intervals at its breakpoints, carried across the
/// `edges` until stable, and makes an atom of every covered interval.
fn atomize(ends: &[End], items: &[Item], edges: &[usize]) -> (Vec<Intervals>, Vec<Atom>) {
    let mut breakpoints = vec![BTreeSet::new(); ends.len()];
    let mut edges_of = vec![Vec::new(); ends.len()];
    let mut work = Vec::new();
    for item in items {
        for b in [item.lsb, item.msb + 1] {
            if breakpoints[item.end].insert(b) {
                work.push((item.end, b));
            }
        }
    }
    for &e in edges {
        let item = &items[e];
        let Intent::Connected { other, .. } = item.intent else {
            unreachable!("edges are connections");
        };
        edges_of[item.end].push(e);
        if other != item.end {
            edges_of[other].push(e);
        }
    }
    while let Some((end, b)) = work.pop() {
        for &e in &edges_of[end] {
            let item = &items[e];
            let Intent::Connected { other, other_lsb } = item.intent else {
                unreachable!("edges are connections");
            };
            let other_msb = other_lsb + (item.msb - item.lsb);
            let sides = [
                (item.end, item.lsb, item.msb, other, other_lsb),
                (other, other_lsb, other_msb, item.end, item.lsb),
            ];
            for (from, lsb, msb, to, to_lsb) in sides {
                if from == end && lsb < b && b <= msb {
                    let mapped = to_lsb + (b - lsb);
                    if breakpoints[to].insert(mapped) {
                        work.push((to, mapped));
                    }
                }
            }
        }
    }

    let mut intervals: Vec<Intervals> = breakpoints
        .iter()
        .map(|bps| {
            let bps: Vec<usize> = bps.iter().copied().collect();
            let ranges: Vec<_> = bps.windows(2).map(|w| (w[0], w[1] - 1)).collect();
            Intervals {
                atoms: vec![None; ranges.len()],
                ranges,
            }
        })
        .collect();
    let mut atoms: Vec<Atom> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let iv = &mut intervals[item.end];
        let mut k = iv.index(item.lsb);
        while k < iv.ranges.len() && iv.ranges[k].1 <= item.msb {
            let atom = *iv.atoms[k].get_or_insert_with(|| {
                atoms.push(Atom {
                    end: item.end,
                    msb: iv.ranges[k].1,
                    lsb: iv.ranges[k].0,
                    items: Vec::new(),
                });
                atoms.len() - 1
            });
            atoms[atom].items.push(i);
            k += 1;
        }
    }
    (intervals, atoms)
}

/// Per end, its maximal uncovered ranges `(msb, lsb)`, MSB first.
fn gaps(ends: &[End], intervals: &[Intervals]) -> Vec<Vec<(usize, usize)>> {
    ends.iter()
        .zip(intervals)
        .map(|(end, iv)| {
            let mut gaps = Vec::new();
            let mut next = end.width; // bits at and above `next` are accounted for
            for (&(lsb, msb), atom) in iv.ranges.iter().zip(&iv.atoms).rev() {
                if atom.is_none() {
                    continue;
                }
                if msb + 1 < next {
                    gaps.push((next - 1, msb + 1));
                }
                next = lsb;
            }
            if next > 0 {
                gaps.push((next - 1, 0));
            }
            gaps
        })
        .collect()
}

/// The components of `core` and the connection diagnostics found on them,
/// ordered by the declaration order of the end each is about, then code.
pub(crate) fn components(core: &Arc<RwLock<ModDefCore>>) -> (Vec<Component>, Vec<Diagnostic>) {
    let Collected {
        def,
        span,
        mut ends,
        stored,
    } = collect(core);
    let (items, mut edges) = items(&mut ends, stored);
    let (intervals, atoms) = atomize(&ends, &items, &edges);

    // Join atoms along connections in call order; a connection between atoms
    // already joined closes a cycle in their component.
    edges.sort_by_key(|&e| items[e].site.seq);
    let mut sets = UnionFind((0..atoms.len()).collect());
    let mut cycles = Vec::new();
    for &e in &edges {
        let item = &items[e];
        let Intent::Connected { other, other_lsb } = item.intent else {
            unreachable!("edges are connections");
        };
        for (lsb, a) in intervals[item.end].atoms_in(item.msb, item.lsb) {
            let mapped = other_lsb + (lsb - item.lsb);
            let b = intervals[other].atoms[intervals[other].index(mapped)]
                .expect("connected ranges are covered");
            if !sets.union(a, b) {
                cycles.push((e, a));
            }
        }
    }

    // Components with their atoms in declaration order of their ends, MSB
    // first; components in the order of their first atoms.
    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for atom in 0..atoms.len() {
        by_root.entry(sets.find(atom)).or_default().push(atom);
    }
    let key = |a: &usize| (atoms[*a].end, Reverse(atoms[*a].msb));
    let mut groups: Vec<(usize, Vec<usize>)> = by_root.into_iter().collect();
    for (_, group) in &mut groups {
        group.sort_by_key(key);
    }
    groups.sort_by_key(|(_, group)| key(&group[0]));
    // The first connection (in call order) that closed a cycle, per
    // component.
    let mut cycle_of: HashMap<usize, usize> = HashMap::new();
    for &(e, atom) in &cycles {
        cycle_of.entry(sets.find(atom)).or_insert(e);
    }

    let ctx = Context {
        def: &def,
        ends: &ends,
        items: &items,
        atoms: &atoms,
    };
    let mut diagnostics = Vec::new();
    let mut components = Vec::new();
    for (root, group) in &groups {
        let (component, found) = ctx.component(group, cycle_of.get(root).copied());
        components.push(component);
        diagnostics.extend(found);
    }
    for (end, gaps) in gaps(&ends, &intervals).into_iter().enumerate() {
        for (msb, lsb) in gaps {
            diagnostics.push(Found {
                end,
                diagnostic: Diagnostic {
                    code: codes::S0402,
                    severity: Severity::Error,
                    message: format!("`{}` is not connected", ctx.end_ref(end, msb, lsb)),
                    primary: Label {
                        span: span.clone(),
                        text: "defined here".into(),
                    },
                    secondary: Vec::new(),
                    notes: Vec::new(),
                    helps: vec!["connect it, tie it off, or mark it unused".into()],
                    object: Some(ctx.object(end, msb, lsb)),
                },
            });
        }
    }
    diagnostics.sort_by_key(|f| (f.end, f.diagnostic.code.0));
    let diagnostics = diagnostics.into_iter().map(|f| f.diagnostic).collect();
    (components, diagnostics)
}

/// A diagnostic and the end its object names, for ordering.
struct Found {
    end: usize,
    diagnostic: Diagnostic,
}

/// A range `(end, msb, lsb)` of an end.
type Range = (usize, usize, usize);

/// How many secondary labels, and S0403 member notes, an error shows.
const SHOWN: usize = 3;

/// What drives a component, before the driver count is checked.
enum Source {
    /// An atom of a definition input or an instance output.
    Port(usize),
    /// An atom and the tieoff item that covers it.
    Constant(usize, usize),
}

struct Context<'a> {
    def: &'a str,
    ends: &'a [End],
    items: &'a [Item],
    atoms: &'a [Atom],
}

impl Context<'_> {
    /// `<definition>.<end>[msb:lsb]`, the range omitted for the whole port.
    fn end_ref(&self, end: usize, msb: usize, lsb: usize) -> String {
        let e = &self.ends[end];
        let range = if lsb == 0 && msb + 1 == e.width {
            String::new()
        } else if msb == lsb {
            format!("[{msb}]")
        } else {
            format!("[{msb}:{lsb}]")
        };
        format!("{}.{}{range}", self.def, e.name())
    }

    fn range_ref(&self, (end, msb, lsb): Range) -> String {
        self.end_ref(end, msb, lsb)
    }

    fn object(&self, end: usize, msb: usize, lsb: usize) -> ObjectRef {
        ObjectRef {
            definition: self.def.to_string(),
            end: self.ends[end].name(),
            msb,
            lsb,
        }
    }

    fn slice(&self, (end, msb, lsb): Range) -> PortSlice {
        PortSlice {
            port: self.ends[end]
                .port
                .clone()
                .expect("an end with atoms has connections"),
            msb,
            lsb,
            select: None,
        }
    }

    /// An error about `range`: the primary label is the involved call made
    /// last, then at most [`SHOWN`] other calls as secondary labels, newest
    /// first, and a note counting the calls left out.
    fn error(
        &self,
        code: Code,
        message: String,
        range: Range,
        involved: &[usize],
        mut notes: Vec<String>,
    ) -> Found {
        let mut calls: Vec<(u64, &str, &Span)> = involved
            .iter()
            .map(|&i| {
                let item = &self.items[i];
                (item.site.seq, item.label(), &item.site.span)
            })
            .collect();
        calls.sort_by(|a, b| (Reverse(a.0), a.1).cmp(&(Reverse(b.0), b.1)));
        calls.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
        match calls.len().saturating_sub(1 + SHOWN) {
            0 => {}
            1 => notes.push("… and 1 more connection".into()),
            more => notes.push(format!("… and {more} more connections")),
        }
        let mut labels = calls
            .into_iter()
            .take(1 + SHOWN)
            .map(|(_, text, span)| Label {
                span: span.clone(),
                text: text.into(),
            });
        let primary = labels.next().expect("an error involves a call");
        let (end, msb, lsb) = range;
        Found {
            end,
            diagnostic: Diagnostic {
                code,
                severity: Severity::Error,
                message,
                primary,
                secondary: labels.collect(),
                notes,
                helps: Vec::new(),
                object: Some(self.object(end, msb, lsb)),
            },
        }
    }

    /// The component of `group` (its atoms, ordered) and its errors; `cycle`
    /// is the first connection that closed a cycle in it.
    fn component(&self, group: &[usize], cycle: Option<usize>) -> (Component, Vec<Found>) {
        // Members: one per atom. Atoms of one end in one component are
        // separate lanes, so adjacent ones are not merged.
        let range = |a: usize| {
            let atom = &self.atoms[a];
            (atom.end, atom.msb, atom.lsb)
        };
        let members: Vec<Range> = group.iter().map(|&a| range(a)).collect();
        // The first member of `end`.
        let range_of = |end: usize| {
            *members
                .iter()
                .find(|m| m.0 == end)
                .expect("the end is a member")
        };
        // The first member that item `i` covers.
        let item_range = |i: usize| {
            group
                .iter()
                .find(|&&a| self.atoms[a].items.contains(&i))
                .map(|&a| range(a))
                .expect("the item covers a member")
        };
        let mut member_ends: Vec<usize> = members.iter().map(|m| m.0).collect();
        member_ends.dedup();
        let first = members[0];
        let mut involved: Vec<usize> = group
            .iter()
            .flat_map(|&a| self.atoms[a].items.iter().copied())
            .collect();
        involved.sort_unstable();
        involved.dedup();
        let kind = |end: usize| self.ends[end].kind;
        let mut errors = Vec::new();

        // Drivers, counted per atom: an atom of a definition input or an
        // instance output, or an atom an allowed tieoff covers (several
        // tieoffs on one atom are S0404, not more drivers).
        let sources: Vec<Source> = group
            .iter()
            .filter_map(|&a| {
                let atom = &self.atoms[a];
                if kind(atom.end).drives() {
                    return Some(Source::Port(a));
                }
                if !kind(atom.end).may_be_tied_off() {
                    return None;
                }
                atom.items
                    .iter()
                    .find(|&&i| self.items[i].tieoff().is_some())
                    .map(|&i| Source::Constant(a, i))
            })
            .collect();
        // The value of tieoff item `i` on atom `a`.
        let constant = |a: usize, i: usize| {
            let (atom, item) = (&self.atoms[a], &self.items[i]);
            item.tieoff()
                .expect("a tieoff item")
                .slice_with_offset_and_width(atom.lsb - item.lsb, atom.msb - atom.lsb + 1)
        };
        let has_inout = member_ends.iter().any(|&end| kind(end).is_inout());

        if sources.len() > 1 {
            let notes = sources
                .iter()
                .map(|source| match *source {
                    Source::Port(a) => format!("driven by `{}`", self.range_ref(range(a))),
                    Source::Constant(a, i) => {
                        let tieoff = constant(a, i);
                        format!(
                            "driven by the constant {}'h{:x}",
                            tieoff.width, tieoff.value
                        )
                    }
                })
                .collect();
            errors.push(self.error(
                codes::S0401,
                format!("`{}` has more than one driver", self.range_ref(first)),
                first,
                &involved,
                notes,
            ));
        }

        let all_unused = group.iter().all(|&a| {
            self.atoms[a]
                .items
                .iter()
                .any(|&i| self.items[i].is_unused())
        });
        if sources.is_empty() && !has_inout && !all_unused {
            let mut notes: Vec<String> = members
                .iter()
                .take(SHOWN)
                .map(|&m| format!("`{}` is on this net", self.range_ref(m)))
                .collect();
            if members.len() > SHOWN {
                notes.push(format!("… and {} more", members.len() - SHOWN));
            }
            errors.push(self.error(
                codes::S0403,
                format!("`{}` has no driver", self.range_ref(first)),
                first,
                &involved,
                notes,
            ));
        }

        if let Some(found) = self.intent_conflict(group, &involved, &member_ends, &item_range) {
            errors.push(found);
        }

        let wrong_direction = involved.iter().find_map(|&i| {
            let item = &self.items[i];
            let k = kind(item.end);
            if item.tieoff().is_some() && !k.may_be_tied_off() {
                let what = if k == Kind::DefInput {
                    "a definition input"
                } else {
                    "an instance output"
                };
                Some((i, format!("cannot be tied off: it is {what}")))
            } else if item.is_unused() && !k.may_be_unused() {
                let what = if k == Kind::DefOutput {
                    "a definition output"
                } else {
                    "an instance input"
                };
                Some((i, format!("cannot be marked unused: it is {what}")))
            } else {
                None
            }
        });
        if let Some((i, why)) = wrong_direction {
            let range = item_range(i);
            errors.push(self.error(
                codes::S0405,
                format!("`{}` {why}", self.range_ref(range)),
                range,
                &[i],
                Vec::new(),
            ));
        }

        // Definition inouts counted per atom: lanes of one inout are separate
        // definition inouts. A component whose only source is a constant has
        // no net, so its inouts do not conflict.
        let def_inouts: Vec<Range> = members
            .iter()
            .copied()
            .filter(|m| kind(m.0) == Kind::DefInOut)
            .collect();
        let constant_only = matches!(sources[..], [Source::Constant(..)]);
        let detail = match def_inouts[..] {
            _ if constant_only => None,
            [] => None,
            [inout] => member_ends
                .iter()
                .find(|&&end| matches!(kind(end), Kind::DefInput | Kind::DefOutput))
                .map(|&end| {
                    let direction = if kind(end) == Kind::DefInput {
                        "input"
                    } else {
                        "output"
                    };
                    format!(
                        "definition inout `{}` with definition {direction} `{}`",
                        self.range_ref(inout),
                        self.range_ref(range_of(end))
                    )
                }),
            _ => {
                let list: Vec<String> = def_inouts
                    .iter()
                    .map(|&inout| format!("`{}`", self.range_ref(inout)))
                    .collect();
                Some(format!("several definition inouts ({})", list.join(", ")))
            }
        };
        if let Some(detail) = detail {
            errors.push(self.error(
                codes::S0406,
                format!("inout conflict on `{}`: {detail}", self.range_ref(first)),
                first,
                &involved,
                Vec::new(),
            ));
        }

        if let Some(e) = cycle {
            errors.push(self.cycle(e, first, &involved));
        }

        let driver = if !errors.is_empty() {
            None
        } else {
            match sources[..] {
                [Source::Port(a)] if kind(self.atoms[a].end) == Kind::DefInput => {
                    Some(Driver::DefInput(self.slice(range(a))))
                }
                [Source::Port(a)] => Some(Driver::InstOutput(self.slice(range(a)))),
                [Source::Constant(a, i)] => Some(Driver::Constant(constant(a, i))),
                _ if has_inout => Some(Driver::Bidirectional(
                    members
                        .iter()
                        .filter(|m| kind(m.0).is_inout())
                        .map(|&m| self.slice(m))
                        .collect(),
                )),
                _ => None,
            }
        };
        let component = Component {
            members: members.iter().map(|&m| self.slice(m)).collect(),
            driver,
        };
        (component, errors)
    }

    /// S0409 for connection `e`, which closed a cycle in the component of
    /// `first`: the ends as the call named them, the call as the primary
    /// label, and the other connections of the component made before it.
    fn cycle(&self, e: usize, first: Range, involved: &[usize]) -> Found {
        let item = &self.items[e];
        let Intent::Connected { other, other_lsb } = item.intent else {
            unreachable!("cycles are closed by connections");
        };
        let here = (item.end, item.msb, item.lsb);
        let there = (other, other_lsb + (item.msb - item.lsb), other_lsb);
        let (this, that) = if item.called_on {
            (here, there)
        } else {
            (there, here)
        };
        let seq = item.site.seq;
        // Connections made before the closing call, one per call, newest
        // first; the closing call stays the primary label.
        let mut others: Vec<usize> = involved
            .iter()
            .copied()
            .filter(|&i| {
                let i = &self.items[i];
                matches!(i.intent, Intent::Connected { .. }) && i.site.seq < seq
            })
            .collect();
        others.sort_by_key(|&i| Reverse(self.items[i].site.seq));
        others.dedup_by_key(|i| self.items[*i].site.seq);
        let mut calls = vec![e];
        calls.extend(others);
        self.error(
            codes::S0409,
            format!(
                "connecting `{}` and `{}` closes a cycle",
                self.range_ref(this),
                self.range_ref(that)
            ),
            first,
            &calls,
            vec!["they are already connected through other connections".into()],
        )
    }

    /// The first S0404 of a component: conflicting intents on one end's
    /// bits, then in the component two net names, a tieoff and a net name,
    /// or a net name on a net driven by a definition input. An unused mark
    /// on an end that may not be marked unused is S0405 instead.
    fn intent_conflict(
        &self,
        group: &[usize],
        involved: &[usize],
        member_ends: &[usize],
        item_range: &dyn Fn(usize) -> Range,
    ) -> Option<Found> {
        let conflict = |a: usize, b: usize, again: bool, range: Range| {
            let (ia, ib) = (&self.items[a], &self.items[b]);
            let second = if again {
                format!("{} again", ib.intent_name())
            } else {
                ib.intent_name().to_string()
            };
            self.error(
                codes::S0404,
                format!(
                    "conflicting intent on `{}`: {} and {second}",
                    self.range_ref(range),
                    ia.intent_name()
                ),
                range,
                &[a, b],
                Vec::new(),
            )
        };
        for &a in group {
            let atom = &self.atoms[a];
            let range = (atom.end, atom.msb, atom.lsb);
            let item = |i: &usize| &self.items[*i];
            let unused: Vec<usize> = atom
                .items
                .iter()
                .copied()
                .filter(|i| item(i).is_unused())
                .collect();
            if let Some(&u) = unused.first()
                && self.ends[atom.end].kind.may_be_unused()
            {
                if let Some(&other) = atom.items.iter().find(|i| !item(i).is_unused()) {
                    return Some(conflict(u, other, false, range));
                }
                if let Some(&again) = unused.get(1) {
                    return Some(conflict(u, again, true, range));
                }
            }
            let tieoffs: Vec<usize> = atom
                .items
                .iter()
                .copied()
                .filter(|i| item(i).tieoff().is_some())
                .collect();
            if let [first, second, ..] = tieoffs[..] {
                return Some(conflict(first, second, true, range));
            }
        }
        let latest = |a: usize, b: usize| {
            if self.items[a].site.seq >= self.items[b].site.seq {
                a
            } else {
                b
            }
        };
        let wires: Vec<usize> = involved
            .iter()
            .copied()
            .filter(|&i| self.items[i].is_wire())
            .collect();
        if let [first, second, ..] = wires[..] {
            return Some(conflict(
                first,
                second,
                true,
                item_range(latest(first, second)),
            ));
        }
        let &wire = wires.first()?;
        if let Some(&tieoff) = involved.iter().find(|&&i| self.items[i].tieoff().is_some()) {
            return Some(conflict(
                tieoff,
                wire,
                false,
                item_range(latest(tieoff, wire)),
            ));
        }
        let &input = member_ends
            .iter()
            .find(|&&end| self.ends[end].kind == Kind::DefInput)?;
        let input = group
            .iter()
            .map(|&a| &self.atoms[a])
            .find(|atom| atom.end == input)
            .map(|atom| (atom.end, atom.msb, atom.lsb))
            .expect("the input is a member");
        let range = item_range(wire);
        Some(self.error(
            codes::S0404,
            format!(
                "a net named on `{}` is driven by definition input `{}`",
                self.range_ref(range),
                self.range_ref(input)
            ),
            range,
            &[wire],
            Vec::new(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::def::resolve_def;
    use super::{Component, Driver};
    use crate::diagnostic::codes;
    use crate::{Code, Diagnostic, IO, ModDef, ModInst, Span, Usage};

    fn leaf(name: &str, port: &str, io: IO) -> ModDef {
        let leaf = ModDef::new(name);
        leaf.add_port(port, io);
        leaf.set_usage(Usage::EmitNothingAndStop);
        leaf
    }

    fn src8() -> ModDef {
        leaf("src8", "o", IO::Output(8))
    }

    fn snk8() -> ModDef {
        leaf("snk8", "dst", IO::Input(8))
    }

    /// An inout hub: receivers connect to each other only through an inout.
    fn io8() -> ModDef {
        leaf("io8", "x", IO::InOut(8))
    }

    fn inst(top: &ModDef, def: &ModDef, name: &str) -> ModInst {
        top.instantiate(def, Some(name), None)
    }

    /// An object reference as (definition, end, msb, lsb).
    type Object = (String, String, usize, usize);

    /// Code, message and object of each diagnostic.
    fn summary(diagnostics: &[Diagnostic]) -> Vec<(Code, String, Object)> {
        diagnostics
            .iter()
            .map(|d| {
                let o = d.object.as_ref().expect("an object reference");
                (
                    d.code,
                    d.message.clone(),
                    (o.definition.clone(), o.end.clone(), o.msb, o.lsb),
                )
            })
            .collect()
    }

    fn object(def: &str, end: &str, msb: usize, lsb: usize) -> Object {
        (def.into(), end.into(), msb, lsb)
    }

    fn line_of(span: &Span) -> u32 {
        match span {
            Span::Rust { file, line, .. } => {
                assert_eq!(file, file!());
                *line
            }
            other => panic!("expected a Rust span, got {other:?}"),
        }
    }

    #[test]
    fn multiply_driven_component_reports_once() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let u = inst(&top, &snk8, "u");
        let a = inst(&top, &src8, "a");
        let b = inst(&top, &src8, "b");
        a.get_port("o").connect(&u.get_port("dst"));
        b.get_port("o").connect(&u.get_port("dst"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.u.dst` has more than one driver".into(),
                object("t", "u.dst", 7, 0)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["driven by `t.a.o`", "driven by `t.b.o`"]
        );
    }

    #[test]
    fn valid_fan_out_has_no_cycle() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let a = inst(&top, &src8, "a");
        let u = inst(&top, &snk8, "u");
        let v = inst(&top, &snk8, "v");
        a.get_port("o").connect(&u.get_port("dst"));
        a.get_port("o").connect(&v.get_port("dst"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(summary(&diagnostics), []);
    }

    #[test]
    fn two_wires_in_a_component_is_s0404() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let a = inst(&top, &src8, "a");
        let u = inst(&top, &snk8, "u");
        a.get_port("o").specify_net_name("x");
        u.get_port("dst").specify_net_name("y");
        a.get_port("o").connect(&u.get_port("dst"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0404,
                "conflicting intent on `t.u.dst`: named a net and named a net again".into(),
                object("t", "u.dst", 7, 0)
            )]
        );
    }

    #[test]
    fn tieoff_and_wire_in_a_component_is_s0404() {
        let io8 = io8();
        let top = ModDef::new("t");
        let y = top.add_port("y", IO::Output(8));
        let w = inst(&top, &io8, "w");
        y.tieoff(0);
        y.connect(&w.get_port("x"));
        w.get_port("x").specify_net_name("n");

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0404,
                "conflicting intent on `t.w.x`: tied off and named a net".into(),
                object("t", "w.x", 7, 0)
            )]
        );
    }

    #[test]
    fn tieoff_on_a_sink_with_no_other_driver_is_valid() {
        let (io8, snk8) = (io8(), snk8());
        let top = ModDef::new("t");
        let w = inst(&top, &io8, "w");
        let u = inst(&top, &snk8, "u");
        w.get_port("x").connect(&u.get_port("dst"));
        u.get_port("dst").tieoff(0x5a);

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert_eq!(summary(&diagnostics), []);
        assert_eq!(resolution.instances.len(), 2, "segments are built");
        assert_eq!(resolution.components.len(), 1);
        let Some(Driver::Constant(tieoff)) = &resolution.components[0].driver else {
            panic!(
                "expected a constant, got {:?}",
                resolution.components[0].driver
            );
        };
        assert_eq!((tieoff.width, tieoff.value.clone()), (8, 0x5a.into()));
    }

    #[test]
    fn tieoff_and_port_driver_is_s0401() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let a = inst(&top, &src8, "a");
        let u = inst(&top, &snk8, "u");
        u.get_port("dst").tieoff(0);
        u.get_port("dst").connect(&a.get_port("o"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.a.o` has more than one driver".into(),
                object("t", "a.o", 7, 0)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["driven by `t.a.o`", "driven by the constant 8'h0"]
        );
    }

    #[test]
    fn tieoffs_on_two_ends_is_s0401() {
        let (io8, snk8) = (io8(), snk8());
        let top = ModDef::new("t");
        let a = inst(&top, &snk8, "a");
        let b = inst(&top, &snk8, "b");
        let w = inst(&top, &io8, "w");
        w.get_port("x").connect(&a.get_port("dst"));
        w.get_port("x").connect(&b.get_port("dst"));
        a.get_port("dst").tieoff(1);
        b.get_port("dst").tieoff(2);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.a.dst` has more than one driver".into(),
                object("t", "a.dst", 7, 0)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["driven by the constant 8'h1", "driven by the constant 8'h2"]
        );
    }

    #[test]
    fn same_end_tied_off_twice_is_s0404() {
        let top = ModDef::new("t");
        let p = top.add_port("p", IO::Output(8));
        p.tieoff(1);
        let line = line!() + 1;
        p.tieoff(2);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0404,
                "conflicting intent on `t.p`: tied off and tied off again".into(),
                object("t", "p", 7, 0)
            )]
        );
        assert_eq!(line_of(&diagnostics[0].primary.span), line);
        assert_eq!(diagnostics[0].primary.text, "tied off here");
        assert_eq!(diagnostics[0].secondary.len(), 1);
        assert_eq!(line_of(&diagnostics[0].secondary[0].span), line - 2);
    }

    #[test]
    fn overlapping_reconnection_is_s0409() {
        let top = ModDef::new("t");
        let out = top.add_port("out", IO::Output(8));
        let a = top.add_port("a", IO::Input(8));
        out.connect(&a);
        let line = line!() + 1;
        out.slice(6, 1).connect(&a.slice(6, 1));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0409,
                "connecting `t.out[6:1]` and `t.a[6:1]` closes a cycle".into(),
                object("t", "out", 6, 1)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["they are already connected through other connections"]
        );
        assert_eq!(line_of(&diagnostics[0].primary.span), line);
    }

    #[test]
    fn cycle_is_reported_in_the_component_it_closes() {
        let top = ModDef::new("t");
        let o = top.add_port("o", IO::Output(8));
        let i = top.add_port("i", IO::Input(8));
        o.slice(7, 4).connect(&i.slice(7, 4));
        o.connect(&i);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0409,
                "connecting `t.o` and `t.i` closes a cycle".into(),
                object("t", "o", 7, 4)
            )]
        );
    }

    #[test]
    fn cycle_labels_show_three_other_connections() {
        let top = ModDef::new("Top");
        let block = ModDef::new("Block");
        block.add_port("x", IO::InOut(1)).unused();
        let b: Vec<ModInst> = (0..5)
            .map(|i| top.instantiate(&block, Some(&format!("b{i}")), None))
            .collect();
        for pair in b.windows(2) {
            pair[0].get_port("x").connect(&pair[1].get_port("x"));
        }
        let line = line!() + 1;
        b[4].get_port("x").connect(&b[0].get_port("x"));
        // A connection made after the closing one is not labelled.
        let extra = top.instantiate(&block, Some("b5"), None);
        extra.get_port("x").connect(&b[2].get_port("x"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0409,
                "connecting `Top.b4.x` and `Top.b0.x` closes a cycle".into(),
                object("Top", "b0.x", 0, 0)
            )]
        );
        let d = &diagnostics[0];
        assert_eq!(line_of(&d.primary.span), line);
        // The three newest of the four other connections.
        let lines: Vec<u32> = d.secondary.iter().map(|l| line_of(&l.span)).collect();
        assert_eq!(lines, [line - 3, line - 3, line - 3]);
        assert_eq!(
            d.notes,
            [
                "they are already connected through other connections",
                "… and 1 more connection"
            ]
        );
    }

    #[test]
    fn later_connections_do_not_take_the_cycle_label() {
        let top = ModDef::new("Top");
        let block = ModDef::new("Block");
        block.add_port("x", IO::InOut(1)).unused();
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| top.instantiate(&block, Some(n), None));
        let ab = line!() + 1;
        a.get_port("x").connect(&b.get_port("x"));
        let bc = line!() + 1;
        b.get_port("x").connect(&c.get_port("x"));
        let line = line!() + 1;
        c.get_port("x").connect(&a.get_port("x"));
        c.get_port("x").connect(&d.get_port("x"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!(d.code, codes::S0409);
        assert_eq!(line_of(&d.primary.span), line);
        let lines: Vec<u32> = d.secondary.iter().map(|l| line_of(&l.span)).collect();
        assert_eq!(lines, [bc, ab]);
        assert_eq!(
            d.notes,
            ["they are already connected through other connections"]
        );
    }

    #[test]
    fn tieoff_on_a_later_lane_is_valid() {
        let (io1, snk3) = (
            leaf("io1", "x", IO::InOut(1)),
            leaf("snk3", "dst", IO::Input(3)),
        );
        let top = ModDef::new("t");
        let w = inst(&top, &io1, "w");
        let u = inst(&top, &snk3, "u");
        w.get_port("x").connect(&u.get_port("dst").bit(0));
        w.get_port("x").connect(&u.get_port("dst").bit(2));
        u.get_port("dst").bit(0).tieoff(0);
        u.get_port("dst").bit(1).tieoff(0);

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert_eq!(summary(&diagnostics), []);
        let Some(Driver::Constant(tieoff)) = &resolution.components[0].driver else {
            panic!("expected a constant: {:?}", resolution.components[0].driver);
        };
        assert_eq!((tieoff.width, tieoff.value.clone()), (1, 0.into()));
        top.emit(crate::EmitOptions::default());
    }

    #[test]
    fn two_lanes_of_a_driver_end_is_s0401() {
        let src2 = leaf("src2", "o", IO::Output(2));
        let snk1 = leaf("snk1", "dst", IO::Input(1));
        let top = ModDef::new("t");
        let u = inst(&top, &snk1, "u");
        let a = inst(&top, &src2, "a");
        u.get_port("dst").connect(&a.get_port("o").bit(0));
        u.get_port("dst").connect(&a.get_port("o").bit(1));

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.u.dst` has more than one driver".into(),
                object("t", "u.dst", 0, 0)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["driven by `t.a.o[1]`", "driven by `t.a.o[0]`"]
        );
        // Lanes of one end stay separate members.
        let s = |end: &str, msb, lsb| (end.to_string(), msb, lsb);
        assert_eq!(
            members(&resolution.components[0]),
            [s("u.dst", 0, 0), s("a.o", 1, 1), s("a.o", 0, 0)]
        );
    }

    #[test]
    fn definition_input_lanes_on_one_output_is_s0401() {
        let top = ModDef::new("t");
        let i = top.add_port("i", IO::Input(2));
        let o = top.add_port("o", IO::Output(1));
        o.connect(&i.bit(0));
        o.connect(&i.bit(1));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.i[1]` has more than one driver".into(),
                object("t", "i", 1, 1)
            )]
        );
    }

    #[test]
    fn tieoff_lanes_of_one_end_is_s0401() {
        let (io1, snk3) = (
            leaf("io1", "x", IO::InOut(1)),
            leaf("snk3", "dst", IO::Input(3)),
        );
        let top = ModDef::new("t");
        let w = inst(&top, &io1, "w");
        let u = inst(&top, &snk3, "u");
        w.get_port("x").connect(&u.get_port("dst").bit(0));
        w.get_port("x").connect(&u.get_port("dst").bit(2));
        u.get_port("dst").tieoff(5);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.w.x` has more than one driver".into(),
                object("t", "w.x", 0, 0)
            )]
        );
        assert_eq!(
            diagnostics[0].notes,
            ["driven by the constant 1'h1", "driven by the constant 1'h1"]
        );
    }

    #[test]
    fn unused_on_a_connected_output_is_only_s0405() {
        let src8 = src8();
        let top = ModDef::new("t");
        let a = inst(&top, &src8, "a");
        let y = top.add_port("y", IO::Output(8));
        y.connect(&a.get_port("o"));
        y.unused();

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0405,
                "`t.y` cannot be marked unused: it is a definition output".into(),
                object("t", "y", 7, 0)
            )]
        );
    }

    #[test]
    fn redundant_triangle_is_s0409() {
        let top = ModDef::new("Top");
        let block = ModDef::new("Block");
        block.add_port("x", IO::InOut(1)).unused();
        let blk_a = top.instantiate(&block, Some("blk_a"), None);
        let blk_b = top.instantiate(&block, Some("blk_b"), None);
        let blk_c = top.instantiate(&block, Some("blk_c"), None);
        blk_a.get_port("x").connect(&blk_b.get_port("x"));
        blk_b.get_port("x").connect(&blk_c.get_port("x"));
        let line = line!() + 1;
        blk_c.get_port("x").connect(&blk_a.get_port("x"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0409,
                "connecting `Top.blk_c.x` and `Top.blk_a.x` closes a cycle".into(),
                object("Top", "blk_a.x", 0, 0)
            )]
        );
        assert_eq!(line_of(&diagnostics[0].primary.span), line);
        assert_eq!(diagnostics[0].primary.text, "connected here");
    }

    #[test]
    fn undriven_net_is_s0403() {
        // Ports that receive connect only to ports that drive or to inouts,
        // so a net without a driver is a named net on receivers alone.
        let snk8 = snk8();
        let top = ModDef::new("t");
        let u = inst(&top, &snk8, "u");
        u.get_port("dst").specify_net_name("n");

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0403,
                "`t.u.dst` has no driver".into(),
                object("t", "u.dst", 7, 0)
            )]
        );
        assert_eq!(diagnostics[0].notes, ["`t.u.dst` is on this net"]);
    }

    #[test]
    fn wrong_direction_tieoff_is_s0405() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let u = inst(&top, &snk8, "u");
        let a = inst(&top, &src8, "a");
        a.get_port("o").connect(&u.get_port("dst"));
        a.get_port("o").tieoff(0);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0405,
                "`t.a.o` cannot be tied off: it is an instance output".into(),
                object("t", "a.o", 7, 0)
            )]
        );
    }

    #[test]
    fn inout_conflict_is_s0406() {
        let top = ModDef::new("t");
        let p = top.add_port("p", IO::InOut(8));
        let q = top.add_port("q", IO::InOut(8));
        p.connect(&q);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0406,
                "inout conflict on `t.p`: several definition inouts (`t.p`, `t.q`)".into(),
                object("t", "p", 7, 0)
            )]
        );
    }

    #[test]
    fn lanes_of_one_definition_inout_are_s0406() {
        let top = ModDef::new("t");
        let x = top.add_port("x", IO::InOut(8));
        x.slice(3, 0).connect(&x.slice(7, 4));

        assert_eq!(
            summary(&top.check().0),
            [(
                codes::S0406,
                "inout conflict on `t.x[7:4]`: several definition inouts (`t.x[7:4]`, `t.x[3:0]`)"
                    .into(),
                object("t", "x", 7, 4)
            )]
        );
    }

    #[test]
    fn definition_inout_lanes_through_an_instance_inout_are_s0406() {
        let io1 = leaf("io1", "x", IO::InOut(1));
        let top = ModDef::new("t");
        let w = inst(&top, &io1, "w");
        let x = top.add_port("x", IO::InOut(2));
        x.bit(0).connect(&w.get_port("x"));
        x.bit(1).connect(&w.get_port("x"));

        assert_eq!(
            summary(&top.check().0),
            [(
                codes::S0406,
                "inout conflict on `t.w.x`: several definition inouts (`t.x[1]`, `t.x[0]`)".into(),
                object("t", "w.x", 0, 0)
            )]
        );
    }

    #[test]
    fn constant_driving_an_inout_and_an_output_is_valid() {
        let top = ModDef::new("t");
        let p = top.add_port("p", IO::InOut(4));
        let y = top.add_port("y", IO::Output(4));
        p.connect(&y);
        y.tieoff(3);

        assert_eq!(summary(&top.check().0), []);
        assert_eq!(
            top.emit(crate::EmitOptions::default()),
            "module t(\n  inout wire [3:0] p,\n  output wire [3:0] y\n);\n  \
             assign p = 4'h3;\n  assign y = 4'h3;\nendmodule\n"
        );
    }

    #[test]
    fn constant_driving_two_definition_inouts_is_valid() {
        let top = ModDef::new("t");
        let p = top.add_port("p", IO::InOut(4));
        let q = top.add_port("q", IO::InOut(4));
        p.connect(&q);
        p.tieoff(3);

        assert_eq!(summary(&top.check().0), []);
    }

    #[test]
    fn s0401_labels_show_three_other_calls() {
        let src1 = leaf("src1", "o", IO::Output(1));
        let snk1 = leaf("snk1", "i", IO::Input(1));
        let top = ModDef::new("t");
        let a = inst(&top, &src1, "a");
        let b = inst(&top, &src1, "b");
        let sinks: Vec<ModInst> = (0..50)
            .map(|k| inst(&top, &snk1, &format!("u{k}")))
            .collect();
        for u in &sinks {
            a.get_port("o").connect(&u.get_port("i"));
        }
        let line = line!() + 1;
        b.get_port("o").connect(&sinks[0].get_port("i"));

        let diagnostics = top.check().0;
        assert_eq!(
            summary(&diagnostics),
            [(
                codes::S0401,
                "`t.a.o` has more than one driver".into(),
                object("t", "a.o", 0, 0)
            )]
        );
        let d = &diagnostics[0];
        assert_eq!(line_of(&d.primary.span), line);
        let lines: Vec<u32> = d.secondary.iter().map(|l| line_of(&l.span)).collect();
        assert_eq!(lines, [line - 3, line - 3, line - 3]);
        assert_eq!(
            d.notes,
            [
                "driven by `t.a.o`",
                "driven by `t.b.o`",
                "… and 47 more connections"
            ]
        );
    }

    #[test]
    fn all_gaps_are_reported() {
        let snk8 = snk8();
        let line = line!() + 1;
        let top = ModDef::new("t");
        let i = top.add_port("i", IO::Input(4));
        let u = inst(&top, &snk8, "u");
        u.get_port("dst").slice(7, 6).connect(&i.slice(3, 2));
        u.get_port("dst").slice(3, 2).connect(&i.slice(1, 0));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [
                (
                    codes::S0402,
                    "`t.u.dst[5:4]` is not connected".into(),
                    object("t", "u.dst", 5, 4)
                ),
                (
                    codes::S0402,
                    "`t.u.dst[1:0]` is not connected".into(),
                    object("t", "u.dst", 1, 0)
                ),
            ]
        );
        for d in &diagnostics {
            assert_eq!(line_of(&d.primary.span), line);
            assert_eq!(d.primary.text, "defined here");
            assert_eq!(d.helps, ["connect it, tie it off, or mark it unused"]);
        }
    }

    #[test]
    fn independent_errors_are_all_reported() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        inst(&top, &snk8, "u");
        let a = inst(&top, &src8, "a");
        a.get_port("o").tieoff(0);

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(
            summary(&diagnostics),
            [
                (
                    codes::S0402,
                    "`t.u.dst` is not connected".into(),
                    object("t", "u.dst", 7, 0)
                ),
                (
                    codes::S0405,
                    "`t.a.o` cannot be tied off: it is an instance output".into(),
                    object("t", "a.o", 7, 0)
                ),
            ]
        );
    }

    #[test]
    fn inputs_and_inouts_drive_components() {
        let (io8, snk8) = (io8(), snk8());
        let top = ModDef::new("t");
        let u = inst(&top, &snk8, "u");
        let w = inst(&top, &io8, "w");
        let i = top.add_port("i", IO::Input(8));
        let p = top.add_port("p", IO::InOut(8));
        i.connect(&u.get_port("dst"));
        p.connect(&w.get_port("x"));

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert_eq!(summary(&diagnostics), []);
        let [input, inout] = &resolution.components[..] else {
            panic!("expected two components: {:?}", resolution.components);
        };
        let Some(Driver::DefInput(driver)) = &input.driver else {
            panic!("expected a definition input, got {:?}", input.driver);
        };
        assert_eq!((driver.port.name(), driver.msb, driver.lsb), ("i", 7, 0));
        let Some(Driver::Bidirectional(inouts)) = &inout.driver else {
            panic!("expected inouts, got {:?}", inout.driver);
        };
        let names: Vec<_> = inouts.iter().map(|s| s.port.name().to_string()).collect();
        assert_eq!(names, ["x", "p"]);
    }

    /// The members of `component` as `(end, msb, lsb)`.
    fn members(component: &Component) -> Vec<(String, usize, usize)> {
        component
            .members
            .iter()
            .map(|m| {
                let end = match m.get_inst_name() {
                    Some(inst) => format!("{inst}.{}", m.port.name()),
                    None => m.port.name().to_string(),
                };
                (end, m.msb, m.lsb)
            })
            .collect()
    }

    #[test]
    fn valid_definition_has_segments_and_components() {
        let (src8, snk8) = (src8(), snk8());
        let top = ModDef::new("t");
        let a = inst(&top, &src8, "a");
        let b = inst(&top, &src8, "b");
        let u = inst(&top, &snk8, "u");
        u.get_port("dst")
            .slice(7, 4)
            .connect(&a.get_port("o").slice(3, 0));
        u.get_port("dst")
            .slice(3, 0)
            .connect(&b.get_port("o").slice(7, 4));
        a.get_port("o").slice(7, 4).unused();
        b.get_port("o").slice(3, 0).unused();

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert_eq!(summary(&diagnostics), []);
        let dst = &resolution.instances[2][0];
        assert_eq!(dst.segments.len(), 2);
        assert_eq!((dst.segments[0].this.msb, dst.segments[0].this.lsb), (7, 4));
        assert_eq!((dst.segments[1].this.msb, dst.segments[1].this.lsb), (3, 0));

        let components: Vec<_> = resolution.components.iter().map(members).collect();
        let s = |end: &str, msb, lsb| (end.to_string(), msb, lsb);
        assert_eq!(
            components,
            [
                vec![s("a.o", 7, 4)],
                vec![s("a.o", 3, 0), s("u.dst", 7, 4)],
                vec![s("b.o", 7, 4), s("u.dst", 3, 0)],
                vec![s("b.o", 3, 0)],
            ]
        );
        // Every component is driven by the instance output it contains,
        // including the two whose only other intent is `unused`.
        for (component, (inst, msb, lsb)) in
            resolution
                .components
                .iter()
                .zip([("a", 7, 4), ("a", 3, 0), ("b", 7, 4), ("b", 3, 0)])
        {
            let Some(Driver::InstOutput(driver)) = &component.driver else {
                panic!("expected an instance output, got {:?}", component.driver);
            };
            assert_eq!(driver.get_inst_name().as_deref(), Some(inst));
            assert_eq!((driver.msb, driver.lsb), (msb, lsb));
        }
    }
}
