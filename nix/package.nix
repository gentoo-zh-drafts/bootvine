{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage (
  import ./bootvine-package-parts.nix { inherit lib; }
  // {
    cargoLock.lockFile = ../Cargo.lock;
    cargoBuildFlags = [
      "--package"
      "bootvine"
    ];
    cargoTestFlags = [
      "--package"
      "bootvine"
    ];
  }
)
