# Connection contract and structural snapshot

Status: accepted, 2026-10-13. Tracks #6 (milestone 2). The serialized schema (JSON Schema and TypeScript types derived from the Rust types) belongs to #8, where the browser consumer lives.

## Goal

Resolve the connectivity of a composition once, in one place, and give every consumer the same result: RTL emission, tracing across hierarchy boundaries, the read-only structural snapshot and structural diagnostics. The connection semantics are Topstitch's; this design states them as a tested contract and gives them one implementation (see [product plan](../product.md), "Architecture").

## Decisions

1. **One design pass.** `resolve_design(top)` (crate-internal) visits the reachable definitions once and returns their resolutions and diagnostics (and, from step 3, their keys). `emit()` and `check()` each consume one such pass, and `elaborate()` will from step 3; no consumer resolves a definition itself.
1. **Nets and segments.** Within a definition, a net is a signal the RTL declares: a definition port, a wire named after an instance port, or a `specify_net_name` wire, indexed by its own bits. Each end (a port of the definition or of one of its instances) is a sequence of MSB-first segments, each mapping a bit range of the end to a range of a net, a constant or `Unused`. Each net range has a driver, kept apart from the net's name. Vectors stay ranges; nothing is bit-blasted.
1. **Errors are diagnostics.** Every connection error is an S04xx diagnostic, reported once per component, not once per end.
1. **Snapshot is a lowering.** `ModDef::elaborate()` lowers the pass into a read-only `Snapshot` keyed by `DefKey`. Nothing mutates it.
1. **Definition keys, not module names.** A definition is identified by a key: its name, plus a hash of its parameter overrides when it has any. Specializations of one module therefore coexist. Two distinct definitions with one key are one definition when equal and an error (S0408) otherwise. Generators choose names; the library never renames.
1. **Independent checks.** Contract tests state their expected connections by hand, in terms of ends and drivers, and check them against the snapshot and, through Slang, against the emitted RTL. Neither check uses the library's net names or normalization.

## Contract

The cases below are executable tests in `tests/contract/`.

| Case | Meaning |
| --- | --- |
| Bit ranges and concatenation | Segments keep explicit bit order. With instance input `u.dst` connected `u.dst[7:4]` to instance output `a.o[3:0]` and `u.dst[3:0]` to instance output `b.o[7:4]`, `u.dst` has two segments whose drivers are `a.o[3:0]` and `b.o[7:4]`. |
| Fan-out and net membership | One driver range may reach several sinks, each taking its own sub-range. All of them are on one net, and the driver of every bit they take is the same. |
| Constants | A tieoff is the constant driver of the bits it covers, with its value and width on the exact range; a partly tied-off end has constant segments beside its other segments. A constant and a port driving the same bits is S0401. |
| Open and unused (from step 2) | `unused()` marks bits intentionally unconnected; it is allowed on definition inputs, instance outputs and inouts, and resolves to an `Unused` segment. Bits of an end neither connected, tied off nor marked unused are S0402. |
| Hierarchy boundaries | An exposed port (`export`, `export_as`) is a definition port on the net of the instance port it exposes. Tracing (from step 3) follows a net out through the definition's ports and into instances of descended definitions, and stops at ports of opaque definitions and of the top. |

Opaque definitions (any `Usage` other than `EmitDefinitionAndDescend`) are boundaries: their ports are endpoints, and no path through them is inferred.

## Resolution

Bits joined by connections within one definition form a connected component, per bit. The components decide drivers and nets; the nets are what segments reference.

**Driver.** For each component, exactly one of:

| Driver | Members |
| --- | --- |
| Definition input | the definition input |
| Instance output | the instance output |
| Constant | a tieoff on any member |
| Bidirectional | only inouts (definition or instance) and no other driver; the driver lists them |

A component with more than one of these is S0401, including two constants (tieoffs on two of its ends overlapping); with none, S0403.

**Net.** A component driven by a constant has no net: every bit of its ends is a `Constant` segment and emits as a literal. Every other component is carried by one net, chosen by the existing rule: the definition input if there is one, else the single definition output or inout, else the instance output (or the first instance inout by instance name, port name and bit range); a `specify_net_name` wire replaces it. A net spans the full width of the port or wire it is named after, so one net can carry several components (`a.o[7:4]` to `b.i` and `a.o[3:0]` to `c.i` are two components on net `a_o`). A port whose only segment is `Unused` has no net; unused bits of a partly connected instance output are on the port's net, with an `Unused` segment. Two name sources of declared nets (the definition ports and the chosen source of each net) whose names are the same string are S0407.

**Segments.** `Resolution` holds, per end in declaration order, MSB-first segments `{ msb, lsb, source }` with `source` one of `Net { net, msb, lsb }`, `Constant { value, width }` or `Unused`. Adjacent segments merge when their sources are contiguous: consecutive ranges of one net, adjacent constants, or adjacent `Unused`. Each component holds its members and driver; each net holds its name and width. Sinks of a net range are the segments that reference it.

**Repeated connections.** A connection identical to an earlier one (the same two ends and ranges, in either order) is ignored. Any other connection between bits already in one component is S0409.

**Emission.** The emitter turns a segment into an expression of the net's emitted name (or a literal), as it does with the prevailing port slice. The emission choices stay in the emitter because they depend on `EmitOptions`: dropping a definition port's segment onto its own net, omitting ports whose only segment is `Unused`, bit-blasting assignments, and slice simplification.

**Locking.** Resolution collects each definition's connections under a short read lock and then traces with no guard held, as `check()` does. Tracing locks cores again, including the definition's own (`get_port_connections`); holding a guard across it can deadlock with a waiting writer.

## Diagnostics

| Code | Meaning |
| --- | --- |
| S0401 | Component bits with more than one driver: two definition inputs or instance outputs, one of each, a constant and a port, or two constants |
| S0402 | Bits of an end that are not connected, tied off or marked unused; names the exact range |
| S0403 | Component bits with no driver (for example a net named with `specify_net_name` on an instance input that nothing drives) |
| S0404 | Conflicting intent on the same bits: unused and connected, tied off or wired; unused twice; the same end tied off twice; wired twice; tied off and wired; a wire on a net driven by a definition input |
| S0405 | Tieoff or unused on an end whose direction does not allow it |
| S0406 | Inout conflict: several definition inouts on one net, or a definition inout with a definition input or output |
| S0407 | Two name sources of declared nets (definition ports, the chosen source of each net) with the same name |
| S0408 | Two different definitions with one key, or an emitted definition and another definition with one module name |
| S0409 | A connection between bits already joined by other connections (a cycle in the connection graph), other than an identical repeat |

Each code has a page in `doc/errors/` and an entry in `cargo xtask explain`. Width mismatches are rejected when a connection is made and are not part of resolution.

**Object reference.** `Diagnostic::object: Option<ObjectRef>` is the stable reference of what a diagnostic is about: the definition key, the end (`port` or `inst.port`) and the bit range. A definition is resolved once however often it is instantiated, so the reference names the definition, not an instance path. For a component-level error the reference is the component's first end in declaration order.

**Spans.** Every connection carries the Rust call site that made it, including `tieoff`, `unused` and `specify_net_name`, and keeps it through slicing and tracing. Every public path to these calls is `#[track_caller]`, so the site is the user's. The primary label is the latest call involved, by order of the calls; at most the three newest other calls involved are secondary labels, and a note counts the rest. S0402 points at the definition's creation site (`ModDef::new` records it); a definition without a recorded site has `Span::None`.

**Scope.** The pass resolves the top when its usage is `EmitDefinitionAndDescend` and every definition reached through instances of descended definitions whose usage is `EmitDefinitionAndDescend`. `check()` returns these diagnostics with its type diagnostics; `emit()` refuses a design with errors.

## Snapshot

`ModDef::elaborate(&self) -> Result<Snapshot, Diagnostics>` returns the snapshot when the pass reports no errors; warnings travel in `Snapshot::diagnostics`.

```rust
pub struct Snapshot {
    pub schema_version: String,                       // SCHEMA_VERSION
    pub top: DefKey,
    pub definitions: IndexMap<DefKey, Definition>,    // reachable definitions
    pub diagnostics: Diagnostics,                     // warnings
}

pub struct Definition {
    pub module: String,              // emitted module name
    pub kind: DefinitionKind,        // Generated | Imported { overrides, key: Option<String> }
    pub usage: Usage,
    pub ports: Vec<PortInfo>,        // name, direction, width, Option<PortType>, Span
    pub interfaces: Vec<Interface>,  // name; members: name, direction (of the port), end, range
    pub instances: Vec<Instance>,    // name, DefKey
    pub ends: Vec<End>,              // segments; descended definitions only
    pub nets: Vec<Net>,              // emitted name, width, drivers; descended only
}
```

- `DefKey` is the module name for a definition without parameter overrides, and `name#<short hash>` for one with them: the `SpecializationKey` short hash (also in the manifest) for a specialization, otherwise a short hash of the overrides (a plain import with `-G` base overrides).
- `PortType`, `Dim`, `Usage` and the other snapshot types derive `Serialize`; serialization format and derived schemas are #8's.
- `SCHEMA_VERSION` starts at `0.1.0`. A consumer accepts a snapshot whose version has the same major and, below `1.0.0`, the same minor version.
- A port's `Span` is its SystemVerilog declaration for an imported port and the Rust call site for a port created by `add_port`, `export` or `export_as`.
- Opaque definitions carry their ports and instances but no `ends` or `nets`.

**Identity.** Definitions are identified by `DefKey`; instances by their path, the instance names from the top joined by dots (`tile_0_0.router`). Renaming or moving an instance changes its path.

**Tracing.** `Snapshot::trace(path, port, msb, lsb) -> Trace` returns, for the given bits of an instance's (or the top's) port, MSB first per bit range: the driver and every member of its net, followed across hierarchy boundaries, each as `(instance path, port, msb, lsb)` or a constant. Inside a definition it follows the net; at a port of an instance of a descended definition it continues on that definition's net; at a definition port it continues in the parent. It stops at ports of opaque definitions and of the top.

## Coexisting definitions

The pass compares every two distinct `ModDef`s that share a `DefKey`. Every definition is compared on its usage, ports (name, direction, width, type, enum cast) and parameter overrides. A stub (`EmitStubAndStop`) is compared on these alone, since its emitted text depends on nothing else. A definition emitted with its contents (`EmitDefinitionAndDescend`) is also compared on its instances (name and child key), interfaces and connections; the connections include the type-check records (whether a connection was made with `connect_bits`) and are compared regardless of call order and direction. A definition whose resolution has errors is never collapsed and is not compared on its connections; its errors are reported. Spans and metadata are provenance and are not compared. Equal definitions are one definition, emitted once, with the first one's provenance; different ones are S0408, naming an instance path that uses each and the first field that differs. An emitted definition and any other definition with another key but the same module name (for example two specializations of one module, both stubbed) are also S0408. Emission takes its set of definitions, children first and in first-seen order, from the same pass.

## Testing

- **Extraction**: the resolver lands with the upstream tests (`tests/emit.rs`, `tests/connections`, `tests/checks`) passing unmodified, including the `validate.rs` 5 s budget.
- **RTL checker**: `tests/common/slang.rs` gains a checker that takes a hand-written map in end terms (`u.dst[7:4]` is driven by `a.o[3:0]`; `u.c` is the constant `8'h5a`; `u.x` is unconnected) and checks it against the emitted text with Slang, without naming nets. For each instance port it reads the connection expression from the text (an empty `.p()` for unconnected); it then evaluates, in a Slang constant function over the module's nets: the driver's expression set one bit at a time, the module's continuous assignments applied, and each sink's expression, which must be exactly the one-hot bit the map names; constants are checked by evaluating the sink expression with all nets zero and with all nets one, which must both give the constant. Unused bits are either an empty `.p()` or bits of a net no other expression of the module reads. A connection through an exposed port is checked as one map per module body along the path, each body elaborated as its own top.
- **Contract**: one test per contract row and one design combining fan-out, a nontrivial slice/concatenation and an exposed nested port. Each hand-written map is checked against the emitted RTL (from step 1) and against the snapshot (from step 3).
- **Diagnostics**: one rendered snapshot test per S04xx code, including its object reference. Upstream tests that expect the panics these codes replace assert the diagnostic.
- **Tracing**: across two levels of hierarchy, through fan-out, stopping at an opaque leaf.
- **Coexistence**: two configurations of one generator under distinct names in one top, emitted and elaborated by Slang; two specializations of one module in one top (the existing typed mesh); two equal definitions under one name emit once; two different ones report S0408.

## Delivery

Sub-issues of #6, landed in order:

1. Resolver and design pass extracted from the emitter, yielding segments (net slices, constants, unused) that the emitter consumes; RTL checker; contract tests on emitted RTL for bit ranges, fan-out, constants and hierarchy. No behavior change: connection errors still panic with their current messages.
1. Components, drivers and nets; `check()` consumes the pass; diagnostics S0401 to S0407 and S0409, `ObjectRef` and call-site spans; identical repeated connections ignored; contract test for open and unused.
1. `Snapshot`, `elaborate()`, `SCHEMA_VERSION` and `trace()`; contract tests extended to the snapshot.
1. `DefKey`, instance paths, coexisting definitions and S0408.
