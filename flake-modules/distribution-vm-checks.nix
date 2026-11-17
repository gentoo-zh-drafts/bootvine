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
        vm-alpine-openrc = import ../vm-tests/alpine-openrc/check.nix {
          inherit lib pkgs;
          staticBootvine = builds.bootvine-static;
        };
        vm-fedora-dracut = import ../vm-tests/fedora-dracut/check.nix {
          inherit lib pkgs;
          staticBootvine = builds.bootvine-static;
        };
      };
    };
}
