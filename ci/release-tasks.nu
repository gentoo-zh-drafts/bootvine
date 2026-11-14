#!/usr/bin/env nu

const CODEBERG_USER = "wvhulle-bot"
const TEA_LOGIN_NAME = "codeberg"
const RELEASE_ARCHIVE_PATTERN = "*.tar.gz"
const CHECKSUM_FILE_NAME = "sha256sum.txt"
const CONVENTIONAL_SUBJECT = '^(build|chore|ci|docs|feat|fix|perf|refactor|revert|style|test)(\([^()]+\))?!?: \S'

def checksum-line [archive_digest: string, archive_name: string]: nothing -> string {
  $"($archive_digest)  ($archive_name)\n"
}

def run-release-plz [subcommand: string]: nothing -> nothing {
  ^release-plz $subcommand --forge gitea --git-token $env.CODEBERG_TOKEN
}

def "main authenticate-git-origin" []: nothing -> nothing {
  ^git remote set-url origin $"https://($CODEBERG_USER):($env.CODEBERG_TOKEN)@codeberg.org/($env.CI_REPO).git"
}

def "main publish-unreleased-crate-version" []: nothing -> nothing {
  main authenticate-git-origin
  ^git checkout -B main $env.CI_COMMIT_SHA
  ^git fetch origin main:refs/remotes/origin/main
  ^git branch --set-upstream-to=origin/main main
  run-release-plz release
}

def "main open-or-update-release-pr" []: nothing -> nothing {
  run-release-plz release-pr
}

def "main collect-release-archives" [
  build_result: path
  dist_directory: path
]: nothing -> nothing {
  mkdir $dist_directory
  for archive_path in (glob ($build_result | path join $RELEASE_ARCHIVE_PATTERN)) {
    cp $archive_path $dist_directory
  }
}

def "main upload-release-assets" [
  dist_directory: path
]: nothing -> nothing {
  let archive_paths = glob ($dist_directory | path join $RELEASE_ARCHIVE_PATTERN) | sort
  let checksum_file = $dist_directory | path join $CHECKSUM_FILE_NAME
  $archive_paths
  | each {|archive_path|
    checksum-line (open --raw $archive_path | hash sha256) ($archive_path | path basename)
  }
  | str join
  | save --force $checksum_file
  ^tea login add --name $TEA_LOGIN_NAME --url $env.CI_FORGE_URL --token $env.CODEBERG_TOKEN
  ^tea releases assets create --login $TEA_LOGIN_NAME --repo $env.CI_REPO $env.CI_COMMIT_TAG ...$archive_paths $checksum_file
}

def "main check-commit-messages" [
  --fetch-target-branch
  ...revision_ranges: string
]: nothing -> nothing {
  if $fetch_target_branch {
    ^git fetch --quiet origin $env.CI_COMMIT_TARGET_BRANCH
  }
  let unconventional_commits = $revision_ranges
  | each {|revision_range|
    let listing = ^git log --no-merges --format=%h%x09%s $revision_range | complete
    if $listing.exit_code != 0 {
      error make { msg: $"git log ($revision_range) failed: ($listing.stderr | str trim)" }
    }
    $listing.stdout | lines
  }
  | flatten
  | parse "{hash}\t{subject}"
  | where subject !~ $CONVENTIONAL_SUBJECT
  if ($unconventional_commits | is-not-empty) {
    let listed_commits = $unconventional_commits | each {|commit| $"  ($commit.hash) ($commit.subject)" } | str join "\n"
    error make {
      msg: $"These commit subjects do not follow Conventional Commits, `type\(scope\)!: description` with type one of build, chore, ci, docs, feat, fix, perf, refactor, revert, style or test:\n($listed_commits)"
    }
  }
}

def "main check-release-pr-is-current" []: nothing -> nothing {
  if (^git rev-parse --is-shallow-repository | str trim) == "true" {
    ^git fetch --quiet --unshallow origin
  }
  ^git fetch --quiet origin $env.CI_COMMIT_TARGET_BRANCH
  let target_branch_head = ^git rev-parse FETCH_HEAD | str trim
  let release_commit_parents = ^git rev-list --parents --max-count=1 HEAD | str trim | split row " " | skip 1
  if $release_commit_parents != [$target_branch_head] {
    error make {
      msg: $"The release pull request is not a single commit on top of ($env.CI_COMMIT_TARGET_BRANCH) at ($target_branch_head), so its changelog misses newer commits. Wait for open-or-update-release-pr to refresh it, or start the release-pr workflow manually."
    }
  }
}

def main []: nothing -> nothing {
  print "Run a subcommand, for example: check-commit-messages"
}
