#!/usr/bin/env nu

const SCREENCAST_TAPES = [
  scripted-demos/tapes/scripted-boot.tape
  scripted-demos/tapes/scripted-shutdown.tape
]
const RECORDINGS_DIRECTORY = "recordings"
const RECORDINGS_BRANCH = "refs/heads/recordings"
const RECORDING_FRAME_MILLISECONDS = 500
const RECORDINGS_COMMIT_MESSAGE = "chore: sample recordings [skip ci]"

def png-frame-size []: path -> record<width: int, height: int> {
  let png_header = open --raw $in | bytes at 16..<24
  {
    width: ($png_header | bytes at 0..<4 | into int --endian big)
    height: ($png_header | bytes at 4..<8 | into int --endian big)
  }
}

def frames-at-final-size []: list<path> -> list<path> {
  let frame_paths = $in
  if ($frame_paths | is-empty) {
    return []
  }
  let final_size = $frame_paths | last | png-frame-size
  $frame_paths | where { ($in | png-frame-size) == $final_size }
}

def encode-machine-recording []: record<name: string, check_output: path> -> nothing {
  let machine_check = $in
  let kept_frames = glob ($machine_check.check_output | path join recording "*.png") | sort | frames-at-final-size
  if ($kept_frames | is-empty) {
    error make {
      msg: $"($machine_check.check_output) has no recording frames"
    }
  }
  let animation_path = $RECORDINGS_DIRECTORY | path join $"($machine_check.name).webp"
  ^img2webp -loop 0 -min_size -mixed -d $RECORDING_FRAME_MILLISECONDS ...$kept_frames -o $animation_path
}

def built-output [installable: string]: nothing -> path {
  ^nix build --no-link --print-out-paths $installable | str trim
}

def sample-machine-names []: nothing -> list<string> {
  ^nix eval --json .#nixosConfigurations --apply builtins.attrNames
  | from json
  | each { str replace --regex '^sample-' '' }
}

def "main record-screencasts" []: nothing -> nothing {
  if not ($SCREENCAST_TAPES | all { path exists }) {
    error make {
      msg: "no VHS tapes here; run from the repository root"
    }
  }
  mkdir $RECORDINGS_DIRECTORY
  let scripted_demos = built-output .#scripted-demos | path join bin
  with-env {PATH: ($env.PATH | prepend $scripted_demos)} {
    for tape_path in $SCREENCAST_TAPES {
      ^vhs $tape_path
    }
  }
  for machine_name in (sample-machine-names) {
    let check_output = built-output $".#checks.x86_64-linux.vm-($machine_name)"
    {name: $machine_name, check_output: $check_output} | encode-machine-recording
  }
}

def "main publish-recordings" []: nothing -> nothing {
  if not ($RECORDINGS_DIRECTORY | path exists) {
    error make {
      msg: "no recordings/ directory here; run record-screencasts from the repository root first"
    }
  }
  let git_directory = ^git rev-parse --absolute-git-dir | str trim
  let temporary_directory = mktemp --directory
  let recordings_index = $temporary_directory | path join recordings-index
  with-env {GIT_INDEX_FILE: $recordings_index GIT_DIR: $git_directory} {
    cd $RECORDINGS_DIRECTORY
    ^git --work-tree=. add --all --force .
    let recordings_tree = ^git write-tree | str trim
    let recordings_commit = ^git commit-tree $recordings_tree -m $RECORDINGS_COMMIT_MESSAGE | str trim
    ^git push --force origin $"($recordings_commit):($RECORDINGS_BRANCH)"
  }
  rm --recursive $temporary_directory
}

def main []: nothing -> nothing {
  help main
}
