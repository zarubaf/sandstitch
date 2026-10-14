#!/bin/bash
# SPDX-License-Identifier: Apache-2.0

# Run inside `nix develop`, which provides Verilator.
set -e

build=$(mktemp -d)
trap 'rm -rf "$build"' EXIT
verilator --binary --timing --top-module demo -Mdir "$build" \
    ../input/adder.sv ../output/top.sv demo.sv
"$build/Vdemo"
