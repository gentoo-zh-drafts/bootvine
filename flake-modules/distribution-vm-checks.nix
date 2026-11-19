{
  perSystem =
    {
      config,
      lib,
      pkgs,
      system,
      ...
    }:
    {
      checks = lib.optionalAttrs (system == "x86_64-linux") {
        vm-alpine-openrc = import ../vm-tests/alpine/check.nix {
          inherit lib pkgs;
          staticBootvine = config.packages.bootvine-static;
        };
        vm-fedora-dracut = import ../vm-tests/fedora/check.nix {
          inherit lib pkgs;
          staticBootvine = config.packages.bootvine-static;
        };
      };
    };
}
