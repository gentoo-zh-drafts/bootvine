#!/usr/bin/env nu

const CONFIGURATION_DOC = "docs/configuration.md"
const DEFAULTS_HEADING = "## Defaults\n"
const TOML_FENCE_OPEN = "```toml\n"
const FENCE_CLOSE = "```\n"

def "main sync-default-config" []: nothing -> nothing {
  let doc_text = open --raw $CONFIGURATION_DOC
  let printed_defaults = ^bootvine defaults | str trim --right
  let heading_start = $doc_text | str index-of $DEFAULTS_HEADING
  if $heading_start < 0 {
    error make {msg: $"($CONFIGURATION_DOC) has no ($DEFAULTS_HEADING | str trim) heading"}
  }
  let after_heading = $doc_text | str substring $heading_start..
  let fence_offset = $after_heading | str index-of $TOML_FENCE_OPEN
  if $fence_offset < 0 {
    error make {msg: $"($CONFIGURATION_DOC) has no toml block under its Defaults heading"}
  }
  let block_start = $heading_start + $fence_offset + ($TOML_FENCE_OPEN | str length)
  let block_end = $block_start + ($doc_text | str substring $block_start.. | str index-of $FENCE_CLOSE)
  let synced_text = [
    ($doc_text | str substring ..<$block_start)
    $printed_defaults
    "\n"
    ($doc_text | str substring $block_end..)
  ] | str join
  if $synced_text != $doc_text {
    $synced_text | save --force $CONFIGURATION_DOC
  }
}

def main []: nothing -> nothing {
  help main
}
