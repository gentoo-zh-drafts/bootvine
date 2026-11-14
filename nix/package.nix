{
  lib,
  rustPlatform,
}:
let
  bootvineManifest = lib.importTOML ../bootvine/Cargo.toml;
  workspaceMemberDirectories = map (memberName: ../. + "/${memberName}") (
    lib.importTOML ../Cargo.toml
  ).workspace.members;
in
rustPlatform.buildRustPackage {
  pname = "bootvine";
  inherit (bootvineManifest.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      (lib.fileset.intersection (lib.fileset.unions workspaceMemberDirectories) (
        lib.fileset.fileFilter (file: file.name == "Cargo.toml" || file.hasExt "rs") ../.
      ))
      ../bootvine/system-files
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [
    "--package"
    "bootvine"
  ];
  cargoTestFlags = [
    "--package"
    "bootvine"
  ];

  strictDeps = true;

  postInstall = ''
    for unit in bootvine.service bootvine-shutdown.service; do
      install -Dm644 bootvine/system-files/systemd/$unit $out/lib/systemd/system/$unit
      substituteInPlace $out/lib/systemd/system/$unit \
        --replace-fail /usr/bin/bootvine $out/bin/bootvine
    done
    resume_drop_in=systemd-hibernate-resume.service.d/bootvine.conf
    install -Dm644 bootvine/system-files/systemd/$resume_drop_in $out/lib/systemd/system/$resume_drop_in
    substituteInPlace $out/lib/systemd/system/$resume_drop_in \
      --replace-fail /usr/bin/bootvine $out/bin/bootvine
    install -Dm755 bootvine/system-files/dracut/90bootvine/module-setup.sh \
      $out/lib/dracut/modules.d/90bootvine/module-setup.sh
    install -Dm644 bootvine/system-files/mkinitcpio/install/sd-bootvine \
      $out/lib/initcpio/install/sd-bootvine
  '';

  meta = {
    description = "Live dependency tree of systemd jobs on the boot console";
    homepage = "https://codeberg.org/wvhulle/bootvine";
    mainProgram = "bootvine";
    platforms = lib.platforms.linux;
    license = lib.getLicenseFromSpdxId bootvineManifest.package.license;
  };
}
