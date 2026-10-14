# SPDX-License-Identifier: Apache-2.0

# The development and CI environment: `nix develop`, then `cargo xtask ci`.
# Every tool comes from the nixpkgs revision pinned in flake.lock.
{
  description = "Sandstitch development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            cmake
            python3
            ccache
            verilator
          ];
          # rust-analyzer and similar tools find the standard library here.
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          # Nix store files share one mtime, so ccache must hash the compiler
          # to notice a compiler change after a flake.lock update.
          CCACHE_COMPILERCHECK = "content";
        };
      });
    };
}
