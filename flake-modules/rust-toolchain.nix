{ inputs, ... }:
{
  perSystem =
    { system, lib, ... }:
    let
      pkgs = import inputs.nixpkgs {
        inherit system;
        overlays = [ (import inputs.rust-overlay) ];
      };
      rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;
      bootvinePackageParts = import ../nix/bootvine-package-parts.nix { inherit lib; };
    in
    {
      _module.args = {
        inherit pkgs rustToolchain bootvinePackageParts;
        craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
        muslCraneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
          rustToolchain.override { targets = [ "x86_64-unknown-linux-musl" ]; }
        );
        craneSourceArgs = {
          inherit (bootvinePackageParts)
            pname
            version
            src
            strictDeps
            ;
        };
      };
    };
}
