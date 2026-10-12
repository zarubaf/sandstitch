# Typed specialization

Status: accepted, 2026-10-12. Tracks #4 (sub-project 1 of 3). Sub-projects 2 (SystemVerilog interface ports) and 3 (bundled struct interfaces) get their own design notes.

## Goal

Let a design package configure an imported SystemVerilog module with value **and type** parameters, keep the resolved port types, check typed connections, and emit readable typed integration RTL. The library stays generic: nothing in it names FlooNoC, PULP or AXI. FlooNoC is the first user, in a separate design crate.

## Decisions

1. **Types come from SystemVerilog, not Rust.** A design package writes a small SV package (typically the IP's own typedef macros with chosen widths) and passes it as a source. Type overrides are SV type expressions, e.g. `cfg_pkg::flit_t`. Slang resolves them; the library does not model type construction. A Rust type builder is deferred until a design needs computed types the IP macros cannot express.
1. **Direct parameterized instances.** Emitted RTL instantiates the original module with `#(...)` overrides. Generated elaboration tops exist only for import and never reach emitted RTL. Upstream `parameterize` already emits direct instances (wrappers come only from `wrap()`); it gains type parameters and a new elaboration path.
1. **Equivalent types connect; bits need intent.** Typed ends must have structurally equivalent types. A plain bit slice or `connect_bits` drops to width-only checking.
1. **Typed nets in emitted RTL** where the type can be named; flat `logic` otherwise.
1. **slang-rs is hard-forked** as a git subtree at `crates/slang-rs` (tag `v0.22.0`) so fixes can be contributed back.
1. **xlsynth-vast is vendored** at `crates/xlsynth-vast` from the published 0.65.0 crate (upstream lives in the xlsynth-crate monorepo, so a subtree is impractical). Needed for type-valued parameters and member selects, which it cannot express today; fixes go upstream as patches.
1. **Fallible variants, panicking wrappers.** New `try_*` functions return `Result<_, ImportError>`; existing upstream functions keep their signatures and panic with the rendered diagnostic. This keeps upstream tests and cherry-picks working.
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

Dependencies: `designs/floonoc → library → crates/{slang-rs, xlsynth-vast}`. `cargo xtask ci` fails if library or `crates/slang-rs` sources mention `floo`, `pulp` or `axi` (case-insensitive, outside tests' inline fixtures). `designs/` and `third_party/` are test and example material: not published, not part of release artifacts. Third-party sources keep their headers and licence files.

## Specialization

A module whose defaults elaborate is imported, then specialized:

```rust
let child = ModDef::try_from_verilog_with_config("child", &cfg)?;
let c4 = child.try_parameterize(&[("N", 4)])?;               // Result<ModDef, ImportError>
```

Parameterized IP often does not elaborate with its defaults: `floo_nw_router`'s type parameters default to `logic` and are used as structs, and `NumRoutes` defaults to 0. Import it directly with its overrides; it is never elaborated with its defaults:

```rust
let r5 = ModDef::try_from_verilog_parameterized::<ParameterValue>(
    "floo_nw_router",
    &cfg,                                                   // sources include cfg_pkg.sv
    &[
        ("NumRoutes", 5.into()),
        ("flit_t", ParameterValue::Type("cfg_pkg::flit_t".into())),
    ],
)?;
```

The result is the same as importing and then calling `try_parameterize` (ports, key, emitted overrides, manifest sources), and it can be parameterized again. Overrides are checked against the parameters the definition declares, read from the AST Slang dumps for the definition alone even when that elaboration fails; Slang's `-G` (`ParserConfig::parameters`) is rejected with S0105, since it applies only to value parameters.

- **Identity**: original module name, ordered overrides, and the import configuration including each source file's content hash and the Slang version. The key has a stable short hash recorded for provenance.
- **Elaboration**: on a cache miss, write a portless top that instantiates the original with the overrides into a temporary directory; Slang elaborates it (with `--ast-json-source-info`); read the instance's ports with resolved types. Slang's JSON leaves ports typed by a type parameter unresolved (`child.T[2:0]`); the slang-rs fork substitutes the instance's `TypeParameter` type and re-applies the dimensions. Value-only overrides use the same path.
- **Result**: a specialized `ModDef` referencing the original module and its overrides; ports carry `PortType`. Specializations of one module coexist; emission uses the original name with different overrides.
- **Errors**: `ImportError` with Slang diagnostics (file, line): unknown type, unknown parameter, unsupported port form.

## Typed ports

`PortType` (library-owned, converted at the slang-rs boundary): `Logic { signed, packed }`, `Struct { name: Option<QualifiedName>, signed, fields, packed }`, `Union { … }`, `Enum { name, base, variants, packed }` (`packed` for arrays of enums such as `state_t [3:0]`). Width is derived from the type. An enum's base is `logic [w-1:0]` with the width and signedness of its variant literals as Slang reports them (`typedef enum {…}` has a signed 32-bit base).

**Equivalence** is structural: same kind, signedness (of vectors, packed structs and unions, and enum bases) and packed dimensions; struct/union field names in order with equivalent field types; enum base and variant names/values. Names are not compared.

| Ends | Rule |
| --- | --- |
| aggregate ↔ aggregate (port or field) | must be equivalent; diagnostic names both types and the first difference |
| either end a logic vector (`PortType::Logic` at the selected level) | widths match (upstream rule: `[8:1]`/`[7:0]`, signedness and dimension shape are not compared) |
| either end a bit slice | widths match |
| `connect_bits(a, b)` | widths match; recorded as intentional reinterpretation |

Logic vectors nested inside aggregates are compared structurally; only a `Logic` end at the selected level makes a connection width-only.

**Access**: `port.field("hdr.dst_id")` (or chained `field`) and `port.index(i)` return typed slices with bit ranges from SV packed layout (first field most significant). They stay compact references.

**Unsupported in this sub-project, with explicit `ImportError`**: unpacked port dimensions, field access through unions, interface ports. No port is silently dropped.

## Emission

- Instances: `mod #(.P(5), .T(cfg_pkg::flit_t)) inst (...)`.
- Nets take the driver's type: `cfg_pkg::flit_t [4:0] n;` when named; `logic [W-1:0]` otherwise.
- Field and index connections emit member selects and indices; `connect_bits` emits a flat assignment.
- The build manifest lists every source the RTL depends on (including generated packages) in compile order.
- Type expressions in `#(...)` go through VAST's expression API, not hand-printed strings.

## Diagnostics

- `Diagnostic { code: "S0xxx", severity, message, primary label, secondary labels, notes, helps }`, spans into Rust (captured with `#[track_caller]`) and SV (Slang locations).
- Rendered with `annotate-snippets` for terminals (Rust spans via `std::panic::Location`, SV spans from `--ast-json-source-info`); serialized to JSON for the explorer, documentation and CI.
- `check()` collects all structural diagnostics; import errors report all Slang diagnostics for that specialization. Upstream `connect` keeps its immediate width/direction panics; type-equivalence violations are reported by `check()`, and `emit()` panics with the rendered diagnostics if `check()` has errors. Converting the remaining upstream panics is later work.
- Error index: one Markdown entry per code with a failing example and the fix; `cargo xtask explain S0xxx`. Tests assert every code has an entry and each example triggers its code.
- Diagnostic output is snapshot-tested on broken fixtures; confusing messages become new fixtures.

## Testing

- Library: inline SV fixtures (no FlooNoC) for type-parameter specialization, coexisting specializations, equivalence, field/index slices, `connect_bits`, unsupported-construct errors, cache-key sensitivity, diagnostic snapshots.
- Round trip: emitted RTL plus original sources elaborate in Slang with zero errors; net widths and types checked against an independently written expected map.
- Design: two FlooNoC router configurations import; an emitted small mesh elaborates in Slang.

## Out of scope

SV interface ports (sub-project 2; its diagnostic lands now), bundled struct interfaces (sub-project 3), the explorer, a Rust type builder, routing (#7).
