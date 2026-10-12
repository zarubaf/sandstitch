# Bundled interfaces

Status: proposed, 2026-10-13. Tracks #4 (sub-project 3 of 3). Builds on [typed specialization](typed-specialization.md); sub-project 2 (SystemVerilog interface ports) is declined.

## Goal

Group struct-typed channel ports of an imported module into named, direction-aware interfaces, and connect, export and tie off whole groups while keeping the typed checks and typed emission of sub-project 1. A grouping is described once on the definition, as references to the imported ports; no port declaration is copied. The library stays generic: FlooNoC is the first user, in `designs/floonoc`.

Today a FlooNoC link is six typed array elements, `floo_{req,rsp,wide}_{o,i}[d]`, wired by hand in `designs/floonoc/src/mesh.rs`: each output element to the neighbour's input element, edge links tied off element by element, local links exported element by element. Topstitch's `Intf` cannot carry this: it stores members as `(port, msb, lsb)` and reads them back as plain bit slices, so connections through it are checked for width only (no S0201), emit flat, and its name matching cannot pair `req` out with `req` in.

## Decisions

1. **One grouping concept.** `Intf` is extended; no second bundle type. Interfaces whose member names are unique behave exactly as today, so upstream tests and ported fixes stay valid.
1. **Typed members.** A member is a name and a `PortSlice` that keeps its typed selection (`floo_req_o[1]` is the element, not bits `[N:M]`). Whole-port members created by the existing constructors are typed whenever their port is.
1. **Direction comes from the port.** A member is keyed by `(name, direction)`; a name may appear twice, once per direction. No roles, modports or name-pairing patterns (`_tx`/`_rx` regexes).
1. **Direction-aware operations** are connect, export and tie-off. The other upstream interface operations keep working on interfaces with unique names and reject the others with a diagnostic.
1. **Membership is model data.** Recording it in the structural snapshot belongs to #6; the explorer to #8.

## Model

`ModDefCore::interfaces` maps an interface name to an ordered list of members. A member stores its name and the typed slice reference: port name, bit range and, when typed, the selection path (`Select` steps). Reading a member returns the same `PortSlice` the caller passed in, including `PortSlice::select`.

Within one interface `(name, direction)` is unique (S0208). Interfaces are defined on a `ModDef` (for FlooNoC, the specialized router) and every instance of it sees them, as today.

New constructor:

```rust
let link = router.def_intf_members(
    format!("link{d}"),
    [
        ("req", router.get_port("floo_req_o").index(d)),
        ("req", router.get_port("floo_req_i").index(d)),
        ("rsp", router.get_port("floo_rsp_o").index(d)),
        ("rsp", router.get_port("floo_rsp_i").index(d)),
        ("wide", router.get_port("floo_wide_o").index(d)),
        ("wide", router.get_port("floo_wide_i").index(d)),
    ],
);
```

Every slice must belong to a port of that `ModDef`; anything else panics, as `def_intf` does for unknown ports. `def_intf`, `def_intf_from_prefix(es)`, `def_intf_from_name_underscore` and `def_intf_from_regex(es)` keep their signatures and build members from their `(port, msb, lsb)` mappings; a range covering the whole port is a typed whole-port member, any other range a plain bit slice, as today.

`Intf::get(name)` and `Intf::remove(name)` return the member of that name; on a name present in both directions they are S0209. `Intf::members()` lists every member as `(name, slice)` in definition order; the slice gives its direction.

## Direction rule

A member's *driving direction* says which side drives it when seen from the module that connects it:

| Interface on | Member port | Drives |
| --- | --- | --- |
| an instance (`Intf::ModInst`) | output | out of the instance |
| an instance | input | into the instance |
| the module being built (`Intf::ModDef`) | input | into the module's body |
| the module being built | output | out of the module's body |

So an input port of the parent drives the parent's body, like an output of a child. This is `PortDirectionality` (`Driver`, `Receiver`, `InOut`). `connect` pairs a driving member with a driven one, and an inout with an inout.

## Operations

| Call | Effect |
| --- | --- |
| `a.connect(&b, allow_mismatch)` | Each member that drives is connected to the same-named member of the other interface that is driven, in both directions. Between instances this pairs an output with an input; between the parent and a child it pairs ports of the same direction (parent input to child input). Every pair is an ordinary typed `connect`. A member without a partner is S0207 unless `allow_mismatch` is `true`; unpaired members are then left alone. |
| `intf.export_with_prefix(name, prefix)` | Exports each member to the parent as port `<prefix><member>`, with the member's type and direction, and defines interface `name` on the parent with the same members. When a name is present in both directions the ports are `<prefix><member>_i` and `<prefix><member>_o`. `export_with_name_underscore` and `export` follow the same rule. |
| `intf.unused_and_tieoff(value)` | Ties off every member driven into the instance and marks every member driven out of it unused. `tieoff` and `unused` alone touch only their own direction. |

Interfaces with unique names keep today's results for all three: when neither interface repeats a name, members pair by name alone, as upstream, so a wrong direction is still the connect-time direction error; exported names gain no suffix.

`connect_except` skips a name in both directions. `connect_pipeline(_except)`, `connect_through(_generic)`, `crossover*`, `subdivide`, `copy_to*`, `flip_to*`, `feedthrough*`, `width`, `iter`, `keys` and `values` are unchanged for interfaces with unique names; on an interface that repeats a name they report S0209 instead of producing a wrong result.

## Diagnostics

Each panics at the Rust call site (`#[track_caller]`) with the rendered diagnostic, like `field` and `index`; each has an error-index entry and an example run by `tests/diagnostics/examples.rs`.

| Code | Reported when |
| --- | --- |
| S0207 | `connect` finds a member without a partner (no same-named member with the opposite driving direction); names the interface, member and direction |
| S0208 | An interface definition repeats `(name, direction)` |
| S0209 | An operation without direction support (or `get`/`remove` by name) is used on an interface that has a name in both directions |

Type mismatches of a pair are S0201 from `ModDef::check`, since each pair is an ordinary typed connection; widths and directions are checked when connecting, as upstream.

## Emission

Nothing new. Interface operations expand into the same typed connections the hand-written code makes, so the net typing rule of sub-project 1 applies unchanged: FlooNoC links stay `cfg_pkg::floo_req_t` nets with element selects.

## Design package

- `router()` defines `link{d}` for every route `d` of the specialized router.
- `mesh_2x2` uses `link(East).connect(&link(West), false)` (and North/South) for neighbours, `unused_and_tieoff(0)` on mesh-edge links and `export_with_prefix` for local links.
- Acceptance: the emitted mesh RTL of both configurations is byte-identical to the RTL emitted before the change, or the PR explains every difference. It still elaborates with its manifest, and the small mesh's links still check against XY coordinates.

## Testing

- Library (`tests/interfaces/`, inline SV fixtures with struct and struct-array ports, no FlooNoC):
  - Peer connect pairs both directions, and parent ↔ child connect keeps directions. Checked in elaborated RTL with distinct bit patterns per member, so a swapped pair fails.
  - Members built from `index`/`field` keep their selection; emitted nets stay typed.
  - S0201 through an interface connection.
  - `export_with_prefix` names, types and directions, with and without repeated names.
  - `unused_and_tieoff` on a bidirectional interface.
  - S0207, S0208 and S0209 diagnostic snapshots.
- Existing interface tests (`tests/interfaces/`) pass unmodified.
- Design (`designs/floonoc/tests/`): the byte-identical comparison above.

## Out of scope

Direction support for the other upstream interface operations (pipelined links, feedthroughs, crossovers); bundles in the snapshot (#6) and explorer (#8); deriving bundles automatically from port names or array dimensions; SystemVerilog interface ports (declined in [typed specialization](typed-specialization.md#systemverilog-interface-ports)).
