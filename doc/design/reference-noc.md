# Reference NoC

## Goal

The working reference NoC of milestone 3 (#7). Rust generators build a mesh of FlooNoC routers whose local ports hold nested compute tiles and memory endpoints. Two system configurations differ in mesh dimensions and core count. The original RTL plus the emitted wrappers elaborate in Slang and simulate in Verilator, and traffic from every core reaches the memory endpoint it addresses.

## Decisions

1. **Address-based XY routing, no tables.**
   - `RouteCfg` uses `XYRouting` with `UseIdTable = 0` and `NumRoutes = 0`.
   - The network interface (`floo_nw_chimney`) takes the destination router from address bits: `x` at `XYAddrOffsetX` and `y` at `XYAddrOffsetY` (`floo_id_translation`). Every endpoint owns the address window whose coordinate bits equal its router's coordinates.
   - `sam_rule_t` is `logic` and `Sam` keeps its default.
   - `port_id` is always 0, so each router has exactly one local port (`num_routes == 5`).
2. **Two packages.**
   - `<name>_pkg` (`cfg_pkg`) holds what the router and the chimney need: the existing types, plus `RouteCfg` and the chimney configurations. It does not depend on the system's address map, so routers of systems with equal router configurations share specializations.
   - `<name>_sys_pkg` holds:
     - one `Window_x<X>_y<Y>` constant per router, the base of its address window, which cores take as their target base;
     - one `MemBase_x<X>_y<Y>` constant per memory endpoint;
     - the core-side AXI types for each core count above one (`axi_mux` widens the ID by `$clog2(N)`, so the cores use `InIdWidth - $clog2(N)` ID bits);
     - the core parameters.

   Both are `PackageDef`s.
3. **Routing artifacts in the manifest.** Chimneys take `RouteCfg`, and cores take their target base, through `PackageDef::constant`. The content-hashed package files are therefore manifest sources that the emitted RTL depends on. The manifest records the routing configuration and the address map.
4. **Chimney configurations.**
   - Compute tiles use `ChimneyCfgNCompute` (`EnMgrPort = 1`, `EnSbrPort = 0`). A request misrouted to a compute tile then fails an assertion.
   - Memory tiles use `ChimneyCfgNMem` (`EnMgrPort = 0`, `EnSbrPort = 1`).
   - Wide channels use `ChimneyDefaultCfg`, with the wide AXI ports tied off or marked unused through the model.
   - `CutRsp` is equal across the narrow and wide configurations.
   - `RouteCfg` spells out every field. Its `CollectiveCfg` is `floo_pkg::CollectiveDefaultCfg`.
   - `MaxAtomicTxns` ≤ `2**OutIdWidth - 1`.
5. **Generators.**
   - `mesh(cfg, &MeshSpec, out_dir)`: an X×Y mesh. Each local port gets an `Endpoint`: `Compute { targets }`, `Memory`, `Export` or a tie-off. `mesh_2x2` is `mesh` with four `Export` endpoints. The mesh rejects a compute target without a memory endpoint, except through a hidden test-only `MeshSpec` override that the misrouted-write simulation test uses.
   - `compute_tile`: one `traffic_core` per target, an `axi_mux` when there is more than one core, and one chimney. Its ports are clock, reset, `id_i`, the floo link interfaces, and `done_o[N]` and `error_o[N]`.
   - `memory_tile`: one chimney whose narrow AXI subordinate side is exported.
   - `system`: the mesh, exporting clock, reset, every memory's AXI subordinate port and every core's `done`/`error`. The system is synthesizable.
6. **Simulation harness (simulation only).** `harness(cfg, &MeshSpec, out_dir)` is a generated `ModDef` and the Verilator top. It instantiates the system and two hand-written fixtures:
   - one `mem_check #(X, Y, ExpectedWrites, ...)` per memory, which wraps `axi_sim_mem` and checks that every monitored write address has its own X and Y bits and that the write count matches;
   - one `tb_driver #(NumCores, TimeoutCycles)`, which drives clock and reset, waits for every `done`, checks `error` and every `mem_check` `ok`, then prints `PASS` or `FAIL` and calls `$finish`.

   Every configuration-dependent value reaches the fixtures as a model parameter.
7. **Core fixture.** `traffic_core` is a synthesizable AXI manager. After reset it writes four beats of `Seed`-derived data to `TargetBase + Seed * 64`, reads them back, compares, and raises `done`, plus `error` on a mismatch or a non-OKAY response. It requires `Seed < 1024`, so its writes stay inside the target's window.
8. **Configurations.**
   - **small:** a 2×2 mesh with one memory and three single-core compute tiles.
   - **large:** a 3×3 mesh with two memories and seven two-core compute tiles. The two cores of a tile target different memories.

   Every route crosses at least two routers, and each configuration has a core at Manhattan distance ≥ 2.
9. **Validation.** `FlooConfig::validate` and the mesh generator reject:
   - `addr_offset_x < 16`, since a core's 64 bytes at `Seed * 64` (decision 7) must stay below the X field;
   - `addr_offset_x + x_bits > addr_offset_y`;
   - `addr_offset_y + y_bits > narrow.addr_width`;
   - `addr_offset_y + y_bits > 32`;
   - `narrow.addr_width != wide.addr_width` (the network interface asserts `AddrWidthMatch`);
   - mesh dimensions above `2^x_bits` or `2^y_bits`;
   - `num_routes != 5` for a mesh with compute or memory endpoints;
   - a core count whose ID widening leaves fewer than one ID bit;
   - `VcImpl::VcCredit`. FlooNoC v0.9.0's `floo_vc_arbiter` connects `cc_credit_counter`'s old `credit_init_i` pin, which the common_cells version it declares calls `clr_i`. Simulation passes `-Wno-PINNOTFOUND` for the unused branch, and the vendored RTL stays unmodified.
10. **Simulation test.** `designs/floonoc/tests/system.rs` emits each harness with its manifest and builds it with:

    ```
    verilator --binary --timing --timescale 1ns/1ps -Wno-fatal -Wno-PINNOTFOUND -j 0
    ```

    It runs the binary and requires `PASS`. Verilator comes from `PATH`, and `nix develop` provides it. When it is missing, the test fails if `CI` is set and otherwise prints why it skipped.
11. **Measurements.**
    - `topstitch::slang_invocations()` counts every Slang run in the process. The test-only per-thread elaboration counter stays.
    - `designs/floonoc/tests/bench.rs` has one `#[ignore]` test per configuration and workload, each run in its own process. A test reports:
      - Slang runs, as a delta;
      - the first (cold) and second (warm) build;
      - check and emit time;
      - peak RSS;
      - the number of distinct router, chimney and core specializations.
    - In the repeated workload all routers share one configuration and all cores one seed. In the unique workload every router gets distinct FIFO depths (through `mesh_with_fifo_depths`) and every core a distinct seed.

## Measurements

Recorded on 2026-10-14 on 16 hardware threads (`nproc`), AMD Ryzen 7 7700 8-Core Processor, in `--release`, with `cargo test -p floonoc --release --test bench -- --ignored --exact <name> --nocapture`, one process per row.

The mesh is built twice in one process into the same directory. The first build is cold; the second is warm, so every specialization is cached. Check and emit time the finished mesh. Slang runs are counted by `slang_invocations()`. Peak RSS is `VmHWM` at the end of the process.

| Test | Cold build | Slang runs (cold) | Warm build | Slang runs (warm) | Check | Emit | Peak RSS | Router / chimney / core specializations |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `small_repeated` | 178 ms | 10 | 9.5 ms | 0 | 7.5 ms | 28 ms | 38 MiB | 1 / 2 / 1 |
| `small_unique` | 288 ms | 15 | 13 ms | 0 | 7.4 ms | 28 ms | 40 MiB | 4 / 2 / 3 |
| `large_repeated` | 212 ms | 14 | 21 ms | 0 | 18 ms | 73 ms | 44 MiB | 1 / 2 / 2 |
| `large_unique` | 532 ms | 34 | 33 ms | 0 | 19 ms | 74 ms | 49 MiB | 9 / 2 / 14 |

The two chimney specializations are the compute and memory roles. In the repeated workload the large configuration has two core specializations, one per memory window, since every core has the same seed but the cores of a tile target different memories.

Verilator (`small_system_passes_traffic`, `large_system_passes_traffic`, with the flags of decision 10):

| Harness | Build | Run | Result |
| --- | --- | --- | --- |
| small (3 cores) | 11.8 s | 10 ms | `PASS` after 42 cycles |
| large (14 cores) | 23.1 s | 10 ms | `PASS` after 87 cycles |

**Budgets.** The Slang run counts of the cold builds, the zero Slang runs of the warm builds and the specialization counts in the table are enforced in CI by `designs/floonoc/tests/budgets.rs`, which runs the four workloads in one process in a fixed order and asserts the exact values. Time and memory budgets are checked only by the ignored bench, which prints `PASS` or `FAIL` for each run: the cold build must take at most twice, and the peak RSS at most 1.5 times, the recorded values. They hold on the recorded machine only.
