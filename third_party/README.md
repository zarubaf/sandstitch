# Third-party RTL

Vendored SystemVerilog used only by tests and examples. It is not part of the
published library, is not a Cargo package, and is not built by Cargo. Each
directory is a `git subtree --squash` import and keeps its upstream licence
files and headers untouched.

| Directory | Upstream | Pinned commit | Licence |
| --- | --- | --- | --- |
| `floonoc/` | https://github.com/pulp-platform/FlooNoC.git | `6ea0648407e7d989b7ec3a1ce6eaa2045b7cd114` (v0.9.0) | SHL-0.51 (`LICENSE-SHL`, `LICENSE-APACHE`) |
| `common_cells/` | https://github.com/pulp-platform/common_cells.git | `63b7c50d43e462b59506f69d341ff1e40202866d` | SHL-0.51 (`LICENSE`) |
| `axi/` | https://github.com/pulp-platform/axi.git | `0ccc838fe06aeeb857eb83c6be9a915c4bf99566` | SHL-0.51 (`LICENSE`) |

SHL-0.51 is the Solderpad Hardware License v0.51, a permissive licence based on
Apache-2.0.

To update a pin:

```sh
git subtree pull --squash --prefix third_party/<name> <url> <new-sha> -m "chore: bump <name>"
```

then update the table above.
