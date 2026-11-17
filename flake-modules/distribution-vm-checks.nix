{
  perSystem =
    {
      lib,
      pkgs,
      system,
      builds,
      ...
    }:
    {
      checks = lib.optionalAttrs (system == "x86_64-linux") {
        vm-alpine-openrc = import ../vm-tests/alpine/check.nix {
          inherit lib pkgs;
          staticBootvine = builds.bootvine-static;
        };
        vm-fedora-dracut = import ../vm-tests/fedora/check.nix {
          inherit lib pkgs;
          staticBootvine = builds.bootvine-static;
        };
      };
    };
}
