{
  pkgs,
  mkApp,
  bootvinePackage,
  scriptedDemos,
  vmCheckOutputs,
  cargoToolchain ? null,
  preCommitInputs ? [ ],
  extraReleaseInputs ? [ ],
  releaseWrapperArgs ? [ ],
}:
let
  inherit (pkgs) lib;

  cargoInputs = lib.optional (cargoToolchain != null) cargoToolchain;

  nuScriptTool =
    {
      script,
      name,
      runtimeInputs ? [ ],
      wrapperArgs ? [ ],
    }:
    pkgs.writers.writeNuBin name {
      makeWrapperArgs = [
        "--add-flags"
        name
      ]
      ++ lib.optionals (runtimeInputs != [ ]) [
        "--prefix"
        "PATH"
        ":"
        (lib.makeBinPath runtimeInputs)
      ]
      ++ wrapperArgs;
    } script;

  nixBuiltPathsTool = toolArgs: nuScriptTool (toolArgs // { script = ../tasks/nix-built-paths.nu; });

  exportBuiltPathsHook = nixBuiltPathsTool { name = "export-built-paths-hook"; };

  exportingBuildApp =
    name:
    mkApp (nixBuiltPathsTool {
      inherit name;
      wrapperArgs = [
        "--set"
        "EXPORT_HOOK_PROGRAM"
        (lib.getExe exportBuiltPathsHook)
      ];
    });

  releasePlzApp =
    name:
    mkApp (nuScriptTool {
      inherit name;
      script = ../tasks/release-tasks.nu;
      runtimeInputs =
        cargoInputs
        ++ [
          pkgs.release-plz
          pkgs.gitMinimal
        ]
        ++ extraReleaseInputs;
      wrapperArgs = releaseWrapperArgs;
    });

  releaseTaskApps =
    lib.mapAttrs
      (
        name: runtimeInputs:
        mkApp (nuScriptTool {
          inherit name runtimeInputs;
          script = ../tasks/release-tasks.nu;
        })
      )
      {
        authenticate-git-origin = [ pkgs.gitMinimal ];
        collect-release-archives = [ ];
        upload-release-assets = [ pkgs.tea ];
        check-commit-messages = [ pkgs.gitMinimal ];
        check-release-pr-is-current = [ pkgs.gitMinimal ];
      };

  preCommitHooksTool =
    pkgs.runCommand "run-pre-commit-hooks"
      {
        nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
        meta.mainProgram = "run-pre-commit-hooks";
      }
      ''
        makeWrapper ${lib.getExe pkgs.prek} $out/bin/run-pre-commit-hooks \
          --add-flags "run --all-files --show-diff-on-failure" \
          --prefix PATH : ${lib.makeBinPath ([ pkgs.gitMinimal ] ++ preCommitInputs)}
      '';
in
releaseTaskApps
// {
  publish-unreleased-crate-version = releasePlzApp "publish-unreleased-crate-version";
  open-or-update-release-pr = releasePlzApp "open-or-update-release-pr";
  open-dependency-update-pr = mkApp (nuScriptTool {
    name = "open-dependency-update-pr";
    script = ../tasks/dependency-updates.nu;
    runtimeInputs = cargoInputs ++ [
      pkgs.gitMinimal
      pkgs.tea
    ];
  });
  run-pre-commit-hooks = mkApp preCommitHooksTool;
  build-and-export-paths = exportingBuildApp "build-and-export-paths";
  build-unless-cached = exportingBuildApp "build-unless-cached";
  import-exported-paths = mkApp (nixBuiltPathsTool {
    name = "import-exported-paths";
  });
  copy-build-output = mkApp (nixBuiltPathsTool {
    name = "copy-build-output";
  });
  upload-exported-paths = mkApp (nixBuiltPathsTool {
    name = "upload-exported-paths";
    runtimeInputs = [ pkgs.cachix ];
  });
  record-screencasts =
    mkApp (nuScriptTool {
      name = "record-screencasts";
      script = ../tasks/recordings.nu;
      runtimeInputs = [
        pkgs.vhs
        pkgs.libwebp
        scriptedDemos
      ];
      wrapperArgs = [
        "--add-flags"
        "${vmCheckOutputs}"
      ];
    })
    // {
      meta.description = "Re-record the scripted demo animations and the sample machine recordings into recordings/; publish them with publish-recordings";
    };
  publish-recordings =
    mkApp (nuScriptTool {
      name = "publish-recordings";
      script = ../tasks/recordings.nu;
      runtimeInputs = [ pkgs.gitMinimal ];
    })
    // {
      meta.description = "Replace the recordings branch on origin with a single commit holding recordings/";
    };
  sync-default-config =
    mkApp (nuScriptTool {
      name = "sync-default-config";
      script = ../tasks/default-config.nu;
      runtimeInputs = [ bootvinePackage ];
    })
    // {
      meta.description = "Rewrite the defaults block in docs/configuration.md from `bootvine defaults`";
    };
}
