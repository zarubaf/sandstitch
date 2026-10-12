# Typed specialization

Status: accepted, 2026-10-12; sub-project 1 implemented. Tracks #4 (sub-project 1 of 3). Sub-project 2 (SystemVerilog interface ports) is declined, see [SystemVerilog interface ports](#systemverilog-interface-ports); sub-project 3 (bundled struct interfaces) gets its own design note.

## Goal

Let a design package configure an imported SystemVerilog module with value **and type** parameters, keep the resolved port types, check typed connections, and emit readable typed integration RTL. The library stays generic: nothing in it names FlooNoC, PULP or AXI. FlooNoC is the first user, in a separate design crate (`designs/floonoc`).

## Decisions

1. **Types come from SystemVerilog, not Rust.** A design package writes a small SV package (typically the IP's own typedef macros with chosen widths) and passes it as a source. Type overrides name SV types, e.g. `cfg_pkg::flit_t`. Slang resolves them; the library does not model type construction. A Rust type builder is deferred until a design needs computed types the IP macros cannot express.
1. **Direct parameterized instances.** Emitted RTL instantiates the original module with `#(...)` overrides. Generated elaboration tops exist only for import and never reach emitted RTL. Wrappers come only from `wrap()`.
1. **Equivalent types connect; bits need intent.** Aggregate ends must have structurally equivalent types. A logic vector, a plain bit slice or `connect_bits` drops to width-only checking.
1. **Typed nets in emitted RTL** where the type can be named and every use is typed; flat vectors otherwise.
1. **slang-rs is hard-forked** as a git subtree at `crates/slang-rs` (tag `v0.22.0`) so fixes can be contributed back.
1. **xlsynth-vast is vendored** at `crates/xlsynth-vast` from the published 0.65.0 crate (upstream lives in the xlsynth-crate monorepo, so a subtree is impractical). Needed for type-valued parameters, member selects and declared packed ranges, which the published crate cannot express; fixes go upstream as patches.
1. **Fallible variants, panicking wrappers.** New `try_*` functions return `Result<_, ImportError>` (`try_manifest`: `Result<_, Diagnostics>`); existing upstream functions keep their signatures and panic with the rendered diagnostic. This keeps upstream tests and ported fixes working.
1. **Diagnostics follow rustc**: codes, spans into Rust call sites and SV sources, notes and help, one model rendered to terminal and JSON.
1. **One entry point**: `cargo xtask` runs every check locally and in CI, replacing pre-commit. Markdown auto-formatting (mdformat, Python) is dropped; text hygiene checks stay.

## Layout

```
/                    library crate (Topstitch fork), workspace root
crates/slang-rs/     slang-rs subtree (hard fork)
crates/xlsynth-vast/ xlsynth-vast 0.65.0, vendored (hard fork)
designs/floonoc/     FlooNoC design package, publish = false
third_party/         pinned RTL subtrees (floonoc, common_cells, axi) with licences
xtask/               cargo xtask: ci, slang, explain
```

Dependencies: `designs/floonoc → library → crates/{slang-rs, xlsynth-vast}`. `cargo xtask ci` fails if `src/` or `crates/*/src/` mention `floo`, `pulp` or `axi` (case-insensitive, as a token prefix for `axi`). `designs/` and `third_party/` are test and example material: not published, not part of release artifacts. Third-party sources keep their headers and licence files.

## Specialization

A module whose defaults elaborate is imported, then specialized:

```rust
let child = ModDef::try_from_verilog_with_config("child", &cfg)?;
let c4 = child.try_parameterize(&[("N", 4)])?;               // Result<ModDef, ImportError>
```

Parameterized IP often does not elaborate with its defaults: `floo_nw_router`'s type parameters default to `logic` and are used as structs, and `NumRoutes` defaults to 0. `try_from_verilog_parameterized` imports it directly with its overrides and does not require the defaults to elaborate: the definition alone is elaborated only to read its declarations, and a failure there is tolerated (`designs/floonoc/src/lib.rs` passes 13 overrides):

```rust
let ty = |name: &str| ParameterValue::Type(format!("cfg_pkg::{name}"));
let r5 = ModDef::try_from_verilog_parameterized::<ParameterValue>(
    "floo_nw_router",
    &cfg,                                                   // sources include cfg_pkg.sv
    &[
        ("AxiCfgN", ParameterValue::Expr("cfg_pkg::AxiCfgN".into())),
        ("NumRoutes", 5.into()),
        ("hdr_t", ty("hdr_t")),
        ("floo_req_t", ty("floo_req_t")),
        // ...
    ],
)?;
```

The result is the same as importing and then calling `try_parameterize` (ports, key, emitted overrides, manifest sources), and it can be parameterized again; overrides merge with the earlier ones.

- **Values**: `ParameterValue::Integer` (non-negative, must fit the parameter's type; emitted as a sized literal), `String` (printable ASCII without quotes or backslashes), `Type` (a type for a `parameter type`) and `Expr` (a named constant for a value parameter). `Type` and `Expr` accept only `ident` or `pkg::ident`; anything else is S0105, as is a value of the wrong kind for its parameter. A bare `ident` refers to the generated top's compilation unit and resolves only with `--single-unit`; prefer `pkg::ident`.
- **Checking**: overrides are checked against the parameters the definition declares, read from the AST Slang dumps for the definition elaborated alone, even when that elaboration fails (as long as the sources parse): S0101 for an undeclared parameter or a localparam. The same pass finds interface ports (S0104), since a module with interface ports cannot be elaborated under a generated top.
- **`-G` values** (`ParserConfig::parameters`) of parameters the module declares are kept: a plain import emits them as `#(...)` overrides of its instances, and every specialization applies them as base overrides that explicit overrides replace. The rules are stated in [S0105](../errors/S0105.md). A specialization never passes `-G` to Slang, which applies it only to tops.
- **Elaboration**: on a cache miss, write a portless top `__sandstitch_elab_<hash>` that instantiates the original with the overrides into a temporary directory; Slang elaborates it with `--ast-json-source-info` and `--Mall` (the files it read); read the instance's ports with resolved types. Slang's JSON leaves ports typed by a type parameter unresolved (`child.T[2:0]`); the slang-rs fork substitutes the instance's `TypeParameter` type and re-applies the dimensions. Value-only overrides use the same path. `cfg.tops` and `cfg.include_hierarchy` do not apply: a specialization has no instances.
- **Identity** (`SpecializationKey`, `ModDef::specialization_key`): original module name, overrides (independent of their order), every field of the import configuration, the Slang version, and the path and content hash of every file Slang read (sources, included headers, library files). Changing any byte of them misses the cache. The key has a stable 12-digit short hash recorded in the manifest.
- **Result**: a specialized `ModDef` referencing the original module and its overrides; ports carry `PortType`. Specializations of one module coexist; emission uses the original name with different overrides.
- **Errors**: S0101 unknown parameter, S0102 Slang elaboration error (all Slang errors of the specialization; errors in the generated top become notes naming the override), S0103 unsupported port, S0104 interface port, S0105 unsupported override. Rust spans point at the caller.

## Typed ports

`PortType` (library-owned, converted at the slang-rs boundary in `src/import/typed.rs`): `Logic { signed, packed }`, `Struct { name: Option<QualifiedName>, signed, is_packed, fields, packed }`, `Union { … }`, `Enum { name, base, variants, packed }` (`packed` for arrays such as `state_t [3:0]`). Width is derived from the type. An enum's base is `logic [w-1:0]` with the width and signedness Slang reports (`typedef enum {…}` has a signed 32-bit base). A typedef'd packed array imports as its element type's name with the typedef's dimensions. Ports added with `add_port`, autoconnect or feedthroughs are untyped (`port_type() == None`); ports created by `export_as`/`export` and by `wrap` keep the type of the port they come from.

**Supported port types**: `logic`/`bit` vectors, the integral atoms (`byte`, `shortint`, `int`, `longint`, `integer`, `time`), packed structs and unions (signed or unsigned), enums, and packed arrays of these. Unpacked structs and unions (`is_packed: false`) are supported only when named with their package and connected whole to an end of the same named type (unpacked types are nominal); any other connection of one is S0206, and `field` on an unpacked struct is S0205. Rejected with a diagnostic, never dropped: unpacked dimensions anywhere in the port type and non-integral types (`real`, `shortreal`, `string`, `event`) are S0103 (skipped instead with `skip_unsupported`); interface ports are S0104.

**Equivalence** is structural: same kind, packedness of structs and unions, signedness (of vectors, packed structs and unions, and enum bases) and packed dimensions (ranges, not just sizes); struct/union field names in order with equivalent field types; enum base and variant names/values. Names are not compared: Slang can report different names for one layout.

| Ends (at the selected level) | Rule |
| --- | --- |
| aggregate ↔ aggregate (struct, union or enum; port, field or element) | must be equivalent; `ModDef::check` reports S0201 naming both types and the first difference |
| either end `PortType::Logic` | widths match (upstream rule: `[8:1]`/`[7:0]`, signedness and dimension shape are not compared) |
| either end a plain bit slice or untyped | widths match |
| `connect_bits(a, b)` | widths match; recorded as intentional reinterpretation |

Logic vectors nested inside aggregates are compared structurally; only a `Logic` end at the selected level makes a connection width-only. Width and direction are checked when the connection is made, as upstream.

**Access**: `port.field("hdr.dst_id")` (or chained `field`) and `port.index(i)` return typed slices with bit ranges from SV packed layout (first field most significant); `index` takes the SV index of the outermost declared dimension. They stay compact references. Errors panic at the call site with the rendered diagnostic: S0202 no such field (including `field` on an array before `index`, or an untyped slice), S0203 index out of range, S0204 field through a union, S0205 field of an unpacked struct.

## Emission

- Instances: `mod #(.P(32'h0000_0005), .T(cfg_pkg::flit_t)) inst (...)`, with type and constant overrides built through VAST's expression API, not hand-printed strings. `emit()` runs `check()` first and panics with the rendered diagnostics if it has errors.
- Typing rule (`src/emit/net_types.rs`): a net, or a port of the emitted module, is declared with the type of the port it is named after when that type is a struct or union named with its package (no negative bounds in its packed dimensions) and every use is typed: whole, or an exact member or element selection, connected to a structurally equivalent end. Such uses are emitted as member selects and indices; declarations keep the declared `[msb:lsb]` ranges (`cfg_pkg::flit_t [5:4] n;`).
- Everything else is flat, with bit slices as upstream: enums and logic vectors, `connect_bits` between different layouts, a struct connected to a vector or an untyped port, a bit range that is no member, wires, and tieoffs (a literal does not assign to every aggregate). Enum input ports keep upstream's casts.
- The build manifest (`ModDef::try_manifest`, `manifest` panics) lists what the emitted RTL needs: `sources` (absolute, deduplicated, compile order; modules whose definition is emitted or stubbed contribute none; text passed to `from_verilog` is not listed), `incdirs`, `defines`, `libfiles`, `libdirs`, `libexts`, and `specializations` (instance path, short hash). S0301: a module emitted from the model whose defining file is also listed would be defined twice. S0302: two listed imports define one macro differently. The emitted RTL plus the manifest elaborate on their own (tested with Slang, strict width warnings).

## Diagnostics

- `Diagnostic { code: "S0xxx", severity, message, primary label, secondary labels, notes, helps }`, spans into Rust (captured with `#[track_caller]`) and SV (Slang locations).
- Rendered with `annotate-snippets` for terminals (Rust spans via `std::panic::Location`, SV spans from `--ast-json-source-info`); serialized to JSON for the explorer, documentation and CI.
- `check()` collects structural diagnostics (S0201, S0206) of the module and the instantiated definitions emitted with `Usage::EmitDefinitionAndDescend` (it does not descend into others); import errors report all Slang diagnostics for that specialization. Upstream `connect` keeps its immediate width/direction panics.
- Error index (`doc/errors/`): one entry per code with a failing example and the fix; `cargo xtask explain S0xxx`. Tests assert every code has an entry, and `tests/diagnostics/examples.rs` runs each example and checks it reports its code.
- Diagnostic output is snapshot-tested on broken fixtures (`tests/diagnostics/snapshots/`).

## Testing

- Library (`tests/typed/`, `tests/diagnostics/`): inline SV fixtures (no FlooNoC) for type-parameter specialization, coexisting specializations, equivalence, field/index slices, `connect_bits`, unsupported-construct errors, cache-key sensitivity, diagnostic snapshots. Emitted RTL is elaborated by Slang with its sources, and typed selections are checked bit by bit.
- Round trip (`tests/roundtrip/`): emitted RTL plus the manifest elaborate in Slang with zero errors; net widths and types are checked against an independently written expected map.
- Design (`designs/floonoc/tests/`): two router configurations import as distinct specializations; a 2x2 mesh of each emits RTL that elaborates with its manifest; the small mesh's links are checked against XY coordinates.

## Known limitations

- S0301 locates a module's file by its first port declaration with an SV span. Not checked: modules without ports, modules none of whose ports has an SV span, and modules whose first such port is declared in an `include`d file (the included file is compared instead of the module's).
- When part of a net is exported, the net is still declared at full width, leaving declared but undriven bits: in `typed_ports_on_the_parent` (`tests/typed/emit.rs`), `s_arr[1]` of `wire cfg_pkg::flit_t [2:0] s_arr` drives the export `arr1` directly and is never driven itself.
- A net whose elements are used in different ways (one element typed, another flat) falls back to a flat net.
- A port typed by a typedef alias of an unpacked struct or union carries the alias's name, so connecting it to the type it names is reported as S0206 although SystemVerilog accepts it (`unpacked_alias_is_reported_although_it_connects`).
- S0206 is over-strict for a re-emitted definition (`Usage::EmitDefinitionAndDescend`) whose unpacked port is tied off inside it: the port is reported even when the parent leaves it unconnected, where the flat declaration would be harmless.
- Only `ident`/`pkg::ident` overrides; computed types and expressions need a named type or constant in a package.

## SystemVerilog interface ports

Declined, 2026-10-13 (sub-project 2 of #4). Interface ports (`interface`/`modport`) stay unsupported and are rejected with S0104, never dropped. No target design needs them: FlooNoC and the IP it imports use struct ports, and outside testbenches the vendored interfaces (`AXI_BUS`, `AXI_LITE`, `cc_stream_dv`) appear only in `*_intf` wrappers around struct-port modules (`axi_xbar_intf` wraps `axi_xbar`). Supporting them would add interface instances to the composition model, the snapshot and emission, with no user to validate them. A design instantiates the struct-port module instead. Revisit when a design must integrate IP that exposes only interface ports.

## Out of scope

SV interface ports (declined, see above), bundled struct interfaces (sub-project 3), the explorer, a Rust type builder, routing (#7).
