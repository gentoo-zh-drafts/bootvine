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
      craneLib = (inputs.crane.mkLib pkgs).overrideToolchain rustToolchain;
      muslCraneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
        rustToolchain.override { targets = [ "x86_64-unknown-linux-musl" ]; }
      );
    in
    {
      _module.args = {
        inherit pkgs rustToolchain craneLib;
        builds = import ../nix/crane-builds.nix {
          inherit lib craneLib muslCraneLib;
        };
      };
    };
}
