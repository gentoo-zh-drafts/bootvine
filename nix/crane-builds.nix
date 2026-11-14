{
  lib,
  pkgs,
  craneLib,
  muslCraneLib,
}:
let
  bootvineCrate = craneLib.crateNameFromCargoToml { cargoToml = ../bootvine/Cargo.toml; };

  commonArgs = {
    inherit (bootvineCrate) pname version;
    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        (craneLib.fileset.commonCargoSources ../.)
        ../bootvine/system-files
        ../.rustfmt.toml
        ../.clippy.toml
      ];
    };
    strictDeps = true;
  };

  cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { pname = "bootvine-workspace"; });

  buildWorkspaceMember =
    args: craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; } // args);
in
{
  inherit cargoArtifacts;

  clippy-lints = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
    }
  );

  bootvine-openrc-static = muslCraneLib.buildPackage {
    inherit (commonArgs) src strictDeps;
    pname = "bootvine-openrc-static";
    inherit (bootvineCrate) version;
    CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
    CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
    cargoExtraArgs = "-p bootvine --no-default-features --features openrc";
    doCheck = false;
  };

  scripted-demos = buildWorkspaceMember {
    inherit (craneLib.crateNameFromCargoToml { cargoToml = ../scripted-demos/Cargo.toml; })
      pname
      version
      ;
    cargoExtraArgs = "-p scripted-demos";
    doCheck = false;
  };

  repo-tasks = buildWorkspaceMember {
    inherit (craneLib.crateNameFromCargoToml { cargoToml = ../xtask/Cargo.toml; }) pname version;
    cargoExtraArgs = "-p xtask";
    doCheck = false;
    meta.mainProgram = "xtask";
  };
}
