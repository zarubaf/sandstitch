// SPDX-License-Identifier: Apache-2.0

//! Resolution of one definition's own connections.

use std::cmp::Reverse;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;

use super::component::components;
use super::{EndResolution, Net, NetName, NetSlice, Resolution, Segment, Source};
use crate::check::connection::check_for_gaps;
use crate::diagnostic::codes;
use crate::model::connection::connected_item::ConnectedItem;
use crate::model::connection::expression_source::merge_expression_sources;
use crate::model::connection::port_slice::CallSite;
use crate::model::connection::port_slice::PortSliceConnections;
use crate::{Diagnostic, IO, Label, ModDefCore, ObjectRef, Port, Severity};

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
            nets: Vec::new(),
        };
        return (resolution, diagnostics);
    }
    let (def_name, instance_jobs, port_jobs, def_ports) = {
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
        let def_ports: Vec<(String, usize)> = core
            .ports
            .iter()
            .map(|(name, io)| (name.clone(), io.width()))
            .collect();
        (core.name.clone(), instance_jobs, port_jobs, def_ports)
    };

    let instances: Vec<Vec<EndResolution>> = instance_jobs
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
    let ports: Vec<EndResolution> = port_jobs
        .into_iter()
        .map(|job| {
            let debug = format!("{def_name}.{}", job.name);
            resolve_end(job, &debug)
        })
        .collect();
    let nets = declared_nets(&def_ports, &instances, &ports);
    let resolution = Resolution {
        instances,
        ports,
        components,
        nets,
    };
    let diagnostics = net_name_collisions(core, &resolution.nets);
    (resolution, diagnostics)
}

/// The nets emission declares, in order of first use: the definition's
/// ports, then what the segments of every instance end and then of every
/// definition port read. The definition's ports are always declared; the
/// net of an instance port, or a named net, that no segment reads is not.
fn declared_nets(
    def_ports: &[(String, usize)],
    instances: &[Vec<EndResolution>],
    ports: &[EndResolution],
) -> Vec<Net> {
    let mut nets: Vec<Net> = Vec::new();
    let mut seen: HashSet<NetName> = HashSet::new();
    let mut add = |name: NetName, width: usize| {
        if seen.insert(name.clone()) {
            nets.push(Net { name, width });
        }
    };
    for (name, width) in def_ports {
        add(NetName::DefPort(name.clone()), *width);
    }
    // An instance end that is only marked unused declares no net; one that
    // also has other segments declares the net of its own port for the
    // unused bits. A definition port never declares one for them.
    let ends = instances
        .iter()
        .flatten()
        .map(|end| (end, true))
        .chain(ports.iter().map(|end| (end, false)));
    for (end, instance) in ends {
        let has_net = end
            .segments
            .iter()
            .any(|s| !matches!(s.source, Source::Unused));
        for segment in &end.segments {
            match &segment.source {
                Source::Net(NetSlice::Port(slice)) => {
                    add(of_port(&slice.port), slice.port.io().width());
                }
                Source::Net(NetSlice::Wire(wire)) => {
                    add(NetName::Wire(wire.name.clone()), wire.width);
                }
                Source::Unused if instance && has_net => {
                    add(of_port(&segment.this.port), segment.this.port.io().width());
                }
                Source::Unused | Source::Constant(_) => {}
            }
        }
    }
    nets
}

/// The net named after `port`.
fn of_port(port: &Port) -> NetName {
    match port.inst_name() {
        Some(inst) => NetName::InstPort(inst.to_string(), port.get_port_name()),
        None => NetName::DefPort(port.get_port_name()),
    }
}

/// S0407 for every net whose emitted name an earlier net already has.
fn net_name_collisions(core: &Arc<RwLock<ModDefCore>>, nets: &[Net]) -> Vec<Diagnostic> {
    let mut first: HashMap<String, &NetName> = HashMap::new();
    let mut collisions = Vec::new();
    for net in nets {
        match first.entry(net.name.emitted()) {
            Entry::Vacant(entry) => {
                entry.insert(&net.name);
            }
            Entry::Occupied(entry) => collisions.push((*entry.get(), net)),
        }
    }
    if collisions.is_empty() {
        return Vec::new();
    }

    // The `specify_net_name` calls, by net name, with the end each was made on.
    let core = core.read();
    let mut wires: HashMap<String, (String, String, usize, CallSite)> = HashMap::new();
    for (inst_name, connections) in &core.mod_inst_connections {
        for (port_name, connections) in connections {
            for c in connections.read().into_iter() {
                if let ConnectedItem::Wire(wire) = &c.other {
                    wires.insert(
                        wire.name.clone(),
                        (
                            inst_name.clone(),
                            port_name.clone(),
                            c.this.port.io().width(),
                            c.site.clone(),
                        ),
                    );
                }
            }
        }
    }

    let describe = |name: &NetName| match name {
        NetName::DefPort(port) => format!("definition port `{port}`"),
        NetName::InstPort(inst, port) => format!("the net of `{inst}.{port}`"),
        NetName::Wire(wire) => format!("net name `{wire}`"),
    };
    collisions
        .into_iter()
        .map(|(earlier, later)| {
            let (end, width) = match &later.name {
                NetName::DefPort(port) => (port.clone(), later.width),
                NetName::InstPort(inst, port) => (format!("{inst}.{port}"), later.width),
                NetName::Wire(wire) => {
                    let (inst, port, width, _) = &wires[wire];
                    (format!("{inst}.{port}"), *width)
                }
            };
            let mut sites: Vec<&CallSite> = [earlier, &later.name]
                .into_iter()
                .filter_map(|name| match name {
                    NetName::Wire(wire) => Some(&wires[wire].3),
                    _ => None,
                })
                .collect();
            sites.sort_by_key(|site| Reverse(site.seq));
            let mut labels = sites.into_iter().map(|site| Label {
                span: site.span.clone(),
                text: "net named here".into(),
            });
            let primary = labels.next().unwrap_or_else(|| Label {
                span: core.span.clone(),
                text: "defined here".into(),
            });
            Diagnostic {
                code: codes::S0407,
                severity: Severity::Error,
                message: format!(
                    "net name `{}` is used by both {} and {}",
                    later.name.emitted(),
                    describe(earlier),
                    describe(&later.name)
                ),
                primary,
                secondary: labels.collect(),
                notes: Vec::new(),
                helps: vec![
                    "rename a port, or give one of the nets another name with `specify_net_name`"
                        .into(),
                ],
                object: Some(ObjectRef {
                    definition: core.name.clone(),
                    end,
                    msb: width - 1,
                    lsb: 0,
                }),
            }
        })
        .collect()
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
    use super::super::{NetName, NetSlice, Source};
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

    /// A definition `name` with one port per `(port, io)`.
    fn module(name: &str, ports: &[(&str, IO)]) -> ModDef {
        let module = ModDef::new(name);
        for (port, io) in ports {
            module.add_port(*port, io.clone());
        }
        module.set_usage(Usage::EmitNothingAndStop);
        module
    }

    fn net_names(resolution: &super::super::Resolution) -> Vec<NetName> {
        resolution.nets.iter().map(|n| n.name.clone()).collect()
    }

    #[test]
    fn split_bus_is_one_net() {
        let a = module("a", &[("o", IO::Output(8))]);
        let b = module("b", &[("i", IO::Input(4))]);
        let c = module("c", &[("i", IO::Input(4))]);
        let top = ModDef::new("t");
        let a = top.instantiate(&a, Some("a"), None);
        let b = top.instantiate(&b, Some("b"), None);
        let c = top.instantiate(&c, Some("c"), None);
        a.get_port("o").slice(7, 4).connect(&b.get_port("i"));
        a.get_port("o").slice(3, 0).connect(&c.get_port("i"));

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            net_names(&resolution),
            [NetName::InstPort("a".into(), "o".into())]
        );
        assert_eq!(resolution.nets[0].width, 8);
    }

    #[test]
    fn wire_named_like_a_port_is_s0407() {
        let (top, a, b, u) = top();
        top.add_port("y", IO::Output(8)).tieoff(0);
        a.get_port("o").connect(&u.get_port("dst"));
        a.get_port("o").specify_net_name("y");
        b.get_port("o").unused();

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d.code, codes::S0407);
        assert_eq!(
            d.message,
            "net name `y` is used by both definition port `y` and net name `y`"
        );
        let o = d.object.as_ref().unwrap();
        assert_eq!(
            (o.definition.as_str(), o.end.as_str(), o.msb, o.lsb),
            ("t", "a.o", 7, 0)
        );
        assert_eq!(d.primary.text, "net named here");
        assert!(d.secondary.is_empty());
    }

    #[test]
    fn auto_names_collide() {
        let a = module("a", &[("b_c", IO::Output(8))]);
        let ab = module("ab", &[("c", IO::Output(8))]);
        let u = module("u", &[("d", IO::Input(8)), ("e", IO::Input(8))]);
        let top = ModDef::new("t");
        let a = top.instantiate(&a, Some("a"), None);
        let ab = top.instantiate(&ab, Some("a_b"), None);
        let u = top.instantiate(&u, Some("u"), None);
        a.get_port("b_c").connect(&u.get_port("d"));
        ab.get_port("c").connect(&u.get_port("e"));

        let (_, diagnostics) = resolve_def(&top.core);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let d = &diagnostics[0];
        assert_eq!(d.code, codes::S0407);
        assert_eq!(
            d.message,
            "net name `a_b_c` is used by both the net of `a.b_c` and the net of `a_b.c`"
        );
        let o = d.object.as_ref().unwrap();
        assert_eq!((o.end.as_str(), o.msb, o.lsb), ("a_b.c", 7, 0));
        assert_eq!(d.primary.text, "defined here");
    }

    #[test]
    fn unchosen_source_does_not_collide() {
        let inout = |name: &str, port: &str| module(name, &[(port, IO::InOut(1))]);
        let top = ModDef::new("t");
        let a = top.instantiate(&inout("a", "b_c_d_e"), Some("a"), None);
        let a_b = top.instantiate(&inout("a_b", "c_d_e"), Some("a_b"), None);
        let a_b_c = top.instantiate(&inout("a_b_c", "d_e"), Some("a_b_c"), None);
        let a_b_c_d = top.instantiate(&inout("a_b_c_d", "e"), Some("a_b_c_d"), None);
        a.get_port("b_c_d_e").connect(&a_b.get_port("c_d_e"));
        a.get_port("b_c_d_e").specify_net_name("xyz");
        a_b_c.get_port("d_e").connect(&a_b_c_d.get_port("e"));

        let (resolution, diagnostics) = resolve_def(&top.core);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let names: Vec<&NetName> = resolution.nets.iter().map(|net| &net.name).collect();
        assert_eq!(
            names,
            [
                &NetName::Wire("xyz".into()),
                &NetName::InstPort("a_b_c".into(), "d_e".into())
            ]
        );
    }
}
