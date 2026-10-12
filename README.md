# Sandstitch

Sandstitch is a reusable library for constructing hardware modules and complete SoCs from existing RTL/IP and generated components. Running a design package resolves a selected configuration and produces integration SystemVerilog, an interactive structural explorer, and module documentation from the same executable design.

See [doc/product.md](doc/product.md) for the product plan.

Imported SystemVerilog modules can be specialized with value and type parameters (`try_parameterize`, or `try_from_verilog_parameterized` for IP whose defaults do not elaborate). Ports keep their resolved struct, union and enum types, connections between them are type-checked, and the emitted RTL uses typed nets plus a build manifest of the sources it needs. See [doc/design/typed-specialization.md](doc/design/typed-specialization.md); [designs/floonoc](designs/floonoc) specializes the FlooNoC router and builds a 2x2 mesh with it. Errors are documented in [doc/errors](doc/errors/README.md).

Forked from [Topstitch](https://github.com/xlsynth/topstitch) v0.100.0 (Apache-2.0), copyright its authors.
