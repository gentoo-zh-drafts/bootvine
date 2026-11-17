{ machine, pkgs }:
{
  name = "bootvine-plasma-session";

  nodes.machine =
    { config, lib, ... }:
    let
      slowSessionExit = pkgs.writeShellScript "slow-session-exit" ''
        trap 'sleep 4; exit' TERM HUP
        while true; do sleep 1; done
      '';
      sessionWithSlowExit = pkgs.writeShellScript "wayland-session-with-slow-exit" ''
        ${slowSessionExit} &
        exec ${config.services.displayManager.sddm.package}/share/sddm/scripts/wayland-session "$@"
      '';
    in
    {
      imports = [ machine ];
      services.displayManager.sddm.settings.Wayland.SessionCommand = toString sessionWithSlowExit;
      virtualisation = {
        memorySize = 4096;
        cores = 4;
      };
      systemd.services = {
        sample-database.serviceConfig.ExecStop = lib.mkForce "/bin/sh -c 'until [ -e /run/release-sample-database ]; do sleep 0.1; done'";
        backdoor.before = [ "sample-database.service" ];
      };
    };

  testScript = builtins.readFile ../shared/console.py + builtins.readFile ./check.py;
}
