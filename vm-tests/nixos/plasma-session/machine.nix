{ lib, pkgs, ... }:
{
  imports = [ ../text-console/machine.nix ];

  services = {
    displayManager = {
      sddm = {
        enable = true;
        wayland.enable = true;
      };
      defaultSession = "plasma";
      autoLogin = {
        enable = true;
        user = "sample-user";
      };
    };
    desktopManager.plasma6 = {
      enable = true;
      enableQt5Integration = false;
    };
    orca.enable = false;
    speechd.enable = false;
    fwupd.enable = false;
    geoclue2.enable = false;
    power-profiles-daemon.enable = false;
    pipewire.enable = false;
  };

  programs.kde-pim.enable = false;

  fonts.enableDefaultPackages = false;

  environment.plasma6.excludePackages = with pkgs.kdePackages; [
    aurorae
    plasma-browser-integration
    plasma-workspace-wallpapers
    konsole
    ark
    elisa
    gwenview
    okular
    kate
    ktexteditor
    khelpcenter
    dolphin
    baloo-widgets
    dolphin-plugins
    spectacle
    ffmpegthumbs
    krdp
    plasma-keyboard
    qtvirtualkeyboard
    union
  ];

  virtualisation.vmVariant.virtualisation = {
    memorySize = lib.mkForce 4096;
    cores = lib.mkForce 4;
  };
}
