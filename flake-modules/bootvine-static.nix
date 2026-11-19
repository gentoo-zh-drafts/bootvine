{
  perSystem =
    { muslCraneLib, craneSourceArgs, ... }:
    {
      packages.bootvine-static = muslCraneLib.buildPackage {
        inherit (craneSourceArgs) src strictDeps version;
        pname = "bootvine-static";
        CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
        CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";
        cargoExtraArgs = "-p bootvine --features openrc";
        doCheck = false;
        meta.mainProgram = "bootvine";
      };
    };
}
