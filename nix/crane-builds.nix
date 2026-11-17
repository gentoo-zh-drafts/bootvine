{
  lib,
  craneLib,
  muslCraneLib,
}:
let
  bootvinePackageParts = import ./bootvine-package-parts.nix { inherit lib; };

  commonArgs = {
    inherit (bootvinePackageParts) pname version src strictDeps;
  };

  bootvine-dependencies = craneLib.buildDepsOnly (
    commonArgs // { pname = "bootvine-dependencies"; }
  );

  buildWorkspaceMember =
    args:
    craneLib.buildPackage (commonArgs // { cargoArtifacts = bootvine-dependencies; } // args);
in
{
  inherit bootvine-dependencies;

  bootvine = buildWorkspaceMember (
    bootvinePackageParts
    // {
      cargoExtraArgs = "-p bootvine";
    }
  );

  bootvine-static = muslCraneLib.buildPackage {
    inherit (commonArgs) src strictDeps version;
    pname = "bootvine-static";
    CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
    CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
    cargoExtraArgs = "-p bootvine --features openrc";
    doCheck = false;
    meta.mainProgram = "bootvine";
  };

  lint-clippy = craneLib.cargoClippy (
    commonArgs
    // {
      cargoArtifacts = bootvine-dependencies;
      cargoClippyExtraArgs = "--workspace --all-targets -- --deny warnings";
    }
  );

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
