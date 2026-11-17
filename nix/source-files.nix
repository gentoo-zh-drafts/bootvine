{ lib }:
let
  workspaceMemberDirectories = map (memberName: ../. + "/${memberName}") (
    lib.importTOML ../Cargo.toml
  ).workspace.members;
in
lib.fileset.toSource {
  root = ../.;
  fileset = lib.fileset.unions [
    ../Cargo.toml
    ../Cargo.lock
    ../.rustfmt.toml
    ../.clippy.toml
    (lib.fileset.intersection (lib.fileset.unions workspaceMemberDirectories) (
      lib.fileset.fileFilter (file: file.name == "Cargo.toml" || file.hasExt "rs") ../.
    ))
    ../bootvine/system-files
  ];
}
