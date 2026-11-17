{ machine, pkgs }:
{
  name = "bootvine-contested-switch";

  nodes.machine =
    { lib, ... }:
    {
      imports = [ machine ];
      systemd.services = {
        sample-database.serviceConfig.ExecStop = lib.mkForce "/bin/sh -c 'until [ -e /run/release-sample-database ]; do sleep 0.1; done'";
        backdoor.before = [ "sample-database.service" ];
        session-terminal-holder = {
          serviceConfig.ExecStart = "${lib.getExe pkgs.python3} ${./hold_session_terminal.py} 2 2";
        };
      };
    };

  testScript = builtins.readFile ../shared/console.py + builtins.readFile ./check.py;
}
