// SPDX-License-Identifier: Apache-2.0

//! Resolution of one definition's own connections.

use std::sync::Arc;

use parking_lot::RwLock;

use super::component::components;
use super::{EndResolution, NetSlice, Resolution, Segment, Source};
use crate::check::connection::check_for_gaps;
use crate::model::connection::connected_item::ConnectedItem;
use crate::model::connection::expression_source::merge_expression_sources;
use crate::model::connection::port_slice::PortSliceConnections;
use crate::{Diagnostic, IO, ModDefCore};

type Connections = Arc<RwLock<PortSliceConnections>>;

/// One port to resolve: its name, direction and width (`io`) and connections (`None` if it has
/// none recorded).
struct PortJob {
    name: String,
    io: IO,
    connections: Option<Connections>,
}

/// Resolves the components of `core` and reports its connection errors; when
/// there are none, resolves every port of every instance of `core`, then
/// every port of `core` itself, into segments.
///
/// Tracing locks the cores of the ports it visits, including `core`, so no
/// guard on `core` is held while resolving.
pub(crate) fn resolve_def(core: &Arc<RwLock<ModDefCore>>) -> (Resolution, Vec<Diagnostic>) {
    let (components, diagnostics) = components(core);
    if !diagnostics.is_empty() {
        let resolution = Resolution {
            instances: Vec::new(),
            ports: Vec::new(),
            components,
        };
        return (resolution, diagnostics);
    }
    let (def_name, instance_jobs, port_jobs) = {
        let core = core.read();
        let instance_jobs: Vec<(String, Vec<PortJob>)> = core
            .instances
            .iter()
            .map(|(inst_name, inst)| {
                let connections = core.mod_inst_connections.get(inst_name);
                let jobs = inst
                    .read()
                    .ports
                    .iter()
                    .map(|(port_name, io)| PortJob {
                        name: port_name.clone(),
                        io: io.clone(),
                        connections: connections.and_then(|c| c.get(port_name)).cloned(),
                    })
                    .collect();
                (inst_name.clone(), jobs)
            })
            .collect();
        let port_jobs: Vec<PortJob> = core
            .ports
            .iter()
            .map(|(port_name, io)| PortJob {
                name: port_name.clone(),
                io: io.clone(),
                connections: core.mod_def_connections.get(port_name).cloned(),
            })
            .collect();
        (core.name.clone(), instance_jobs, port_jobs)
    };

    let instances = instance_jobs
        .into_iter()
        .map(|(inst_name, jobs)| {
            jobs.into_iter()
                .map(|job| {
                    let debug = format!("{def_name}.{inst_name}.{}", job.name);
                    resolve_end(job, &debug)
                })
                .collect()
        })
        .collect();
    let ports = port_jobs
        .into_iter()
        .map(|job| {
            let debug = format!("{def_name}.{}", job.name);
            resolve_end(job, &debug)
        })
        .collect();
    let resolution = Resolution {
        instances,
        ports,
        components,
    };
    (resolution, diagnostics)
}

/// The segments of one end; `debug` names it in panic messages.
fn resolve_end(job: PortJob, debug: &str) -> EndResolution {
    let Some(connections) = job.connections else {
        panic!("{debug} is unconnected");
    };

    // break into non-overlapping chunks
    let mut non_overlapping = connections.read().trace().make_non_overlapping();

    non_overlapping.retain(|c| !c.is_empty());
    non_overlapping.sort_by_key(|c| -(c[0].this.msb as isize));

    // make sure there aren't gaps between connections for this port
    check_for_gaps(&non_overlapping, &job.io, debug);

    let expression_sources = non_overlapping
        .iter()
        .map(|c| c.to_expression_source().unwrap())
        .collect::<Vec<_>>();

    let segments = merge_expression_sources(expression_sources)
        .into_iter()
        .map(|c| Segment {
            this: c.this,
            source: match c.other {
                ConnectedItem::PortSlice(slice) => Source::Net(NetSlice::Port(slice)),
                ConnectedItem::Wire(wire) => Source::Net(NetSlice::Wire(wire)),
                ConnectedItem::Tieoff(tieoff) => Source::Constant(tieoff),
                ConnectedItem::Unused(_) => Source::Unused,
            },
        })
        .collect();
    EndResolution {
        name: job.name,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{NetSlice, Source};
    use super::resolve_def;
    use crate::diagnostic::codes;
    use crate::{IO, ModDef, ModInst, Usage};

    fn leaf(name: &str, port: &str, io: IO) -> ModDef {
        let leaf = ModDef::new(name);
        leaf.add_port(port, io);
        leaf.set_usage(Usage::EmitNothingAndStop);
        leaf
    }

    /// `t` with sources `a`, `b` (`o`, 8 bits) and a sink `u` (`dst`, 8 bits).
    fn top() -> (ModDef, ModInst, ModInst, ModInst) {
        let src8 = leaf("src8", "o", IO::Output(8));
        let snk8 = leaf("snk8", "dst", IO::Input(8));
        let top = ModDef::new("t");
        let a = top.instantiate(&src8, Some("a"), None);
        let b = top.instantiate(&src8, Some("b"), None);
        let u = top.instantiate(&snk8, Some("u"), None);
        (top, a, b, u)
    }

    #[test]
    fn segments_are_msb_first() {
        let (top, a, b, u) = top();
        u.get_port("dst")
            .slice(7, 4)
            .connect(&a.get_port("o").slice(3, 0));
        u.get_port("dst")
            .slice(3, 0)
            .connect(&b.get_port("o").slice(7, 4));
        a.get_port("o").slice(7, 4).unused();
        b.get_port("o").slice(3, 0).unused();

        let (resolution, _) = resolve_def(&top.core);
        let dst = &resolution.instances[2][0];
        assert_eq!(dst.name, "dst");
        assert_eq!(dst.segments.len(), 2);
        let (hi, lo) = (&dst.segments[0], &dst.segments[1]);
        assert_eq!((hi.this.msb, hi.this.lsb), (7, 4));
        assert_eq!((lo.this.msb, lo.this.lsb), (3, 0));
        let Source::Net(NetSlice::Port(s)) = &hi.source else {
            panic!("expected a port source, got {:?}", hi.source);
        };
        assert_eq!(s.get_inst_name().as_deref(), Some("a"));
        assert_eq!(s.port.get_port_name(), "o");
        assert_eq!((s.msb, s.lsb), (3, 0));
        let Source::Net(NetSlice::Port(s)) = &lo.source else {
            panic!("expected a port source, got {:?}", lo.source);
        };
        assert_eq!(s.get_inst_name().as_deref(), Some("b"));
        assert_eq!((s.msb, s.lsb), (7, 4));
    }

    #[test]
    fn contiguous_segments_merge() {
        let (top, a, b, u) = top();
        u.get_port("dst")
            .slice(7, 4)
            .connect(&a.get_port("o").slice(7, 4));
        u.get_port("dst")
            .slice(3, 0)
            .connect(&a.get_port("o").slice(3, 0));
        b.get_port("o").unused();

        let (resolution, _) = resolve_def(&top.core);
        let dst = &resolution.instances[2][0];
        assert_eq!(dst.segments.len(), 1);
        let seg = &dst.segments[0];
        assert_eq!((seg.this.msb, seg.this.lsb), (7, 0));
        let Source::Net(NetSlice::Port(s)) = &seg.source else {
            panic!("expected a port source, got {:?}", seg.source);
        };
        assert_eq!(s.get_inst_name().as_deref(), Some("a"));
        assert_eq!(s.port.get_port_name(), "o");
        assert_eq!((s.msb, s.lsb), (7, 0));
    }

    #[test]
    fn tieoff_and_unused_are_sources() {
        let (top, a, b, u) = top();
        u.get_port("dst").slice(7, 4).tieoff(5);
        u.get_port("dst")
            .slice(3, 0)
            .connect(&a.get_port("o").slice(3, 0));
        a.get_port("o").slice(7, 4).unused();
        b.get_port("o").unused();

        let (resolution, _) = resolve_def(&top.core);
        let dst = &resolution.instances[2][0];
        let Source::Constant(t) = &dst.segments[0].source else {
            panic!("expected a constant, got {:?}", dst.segments[0].source);
        };
        assert_eq!(t.value, 5.into());
        assert_eq!(t.width, 4);
        assert_eq!(dst.segments.len(), 2);
        let this = &dst.segments[0].this;
        assert_eq!((this.msb, this.lsb), (7, 4));

        let b_o = &resolution.instances[1][0];
        assert_eq!(b_o.segments.len(), 1);
        assert!(matches!(b_o.segments[0].source, Source::Unused));
    }

    #[test]
    fn instance_ports_resolve_before_definition_ports() {
        let snk8 = leaf("snk8", "dst", IO::Input(8));
        let top = ModDef::new("t");
        top.add_port("out", IO::Output(8));
        top.instantiate(&snk8, Some("u"), None);
        let (resolution, diagnostics) = resolve_def(&top.core);
        let found: Vec<_> = diagnostics
            .iter()
            .map(|d| {
                let o = d.object.as_ref().unwrap();
                (d.code, o.definition.as_str(), o.end.as_str(), o.msb, o.lsb)
            })
            .collect();
        assert_eq!(
            found,
            [
                (codes::S0402, "t", "u.dst", 7, 0),
                (codes::S0402, "t", "out", 7, 0)
            ]
        );
        assert!(resolution.instances.is_empty() && resolution.ports.is_empty());
    }
}
