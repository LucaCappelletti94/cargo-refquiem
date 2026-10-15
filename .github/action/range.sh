#!/usr/bin/env bash
# Resolves the commit range the triggering event scans and fetches the history it needs.
# Writes `base`, `head` and `since` to GITHUB_OUTPUT, an empty base scanning the full history.
set -euo pipefail

zero=0000000000000000000000000000000000000000
event="$(cat "$GITHUB_EVENT_PATH")"
field() { jq -r "$1 // empty" <<< "$event"; }

fetch() { git fetch -q --no-tags origin "$@"; }

# The full history and the tags, which an audit or a range without a common ancestor reads.
fetch_all() {
  if [ "$(git rev-parse --is-shallow-repository)" = true ]; then
    git fetch -q --tags --unshallow origin
  else
    git fetch -q --tags origin
  fi
}

# Fetches `$1` alone when it is missing, failing when the remote no longer has it.
fetch_commit() {
  git cat-file -e "$1^{commit}" 2>/dev/null || fetch --depth=1 "$1"
}

# Deepens a shallow clone from the given refspecs until `$1` and `$2` share an ancestor,
# then prints their merge base, or nothing when the full histories share none.
deepen_to_merge_base() {
  local one="$1" two="$2" depth=64
  shift 2
  while ! git merge-base "$one" "$two" 2>/dev/null; do
    if [ "$(git rev-parse --is-shallow-repository)" != true ]; then
      return 0
    fi
    if [ "$depth" -gt 4096 ]; then
      git fetch -q --no-tags --unshallow origin "$@"
      git merge-base "$one" "$two" 2>/dev/null || true
      return 0
    fi
    fetch --deepen="$depth" "$@"
    depth=$((depth * 2))
  done
}

base=''
head=''
since=''
case "$GITHUB_EVENT_NAME" in
  pull_request | pull_request_target | merge_group)
    if [ "$GITHUB_EVENT_NAME" = merge_group ]; then
      base="$(field .merge_group.base_sha)"
      head="$(field .merge_group.head_sha)"
    else
      base="$(field .pull_request.base.sha)"
      head="$GITHUB_SHA"
    fi
    fetch_commit "$base"
    fetch_commit "$head"
    if [ -z "$(deepen_to_merge_base "$base" "$head" "$base" "$head")" ]; then
      fetch_all
    fi
    ;;
  push)
    before="$(field .before)"
    head="$(field .after)"
    fetch_commit "$head"
    # A new branch, or an old tip the remote dropped, starts from the default branch.
    other="$before"
    if [ "$before" = "$zero" ] || ! fetch_commit "$before" 2>/dev/null; then
      default="$(field .repository.default_branch)"
      fetch --depth=1 "+refs/heads/$default:refs/remotes/origin/$default"
      other="refs/remotes/origin/$default"
      refspec="+refs/heads/$default:refs/remotes/origin/$default"
    else
      refspec="$before"
    fi
    # The merge base is the old tip after a fast forward and the fork point after a force push.
    base="$(deepen_to_merge_base "$other" "$head" "$refspec" "$head")"
    if [ -z "$base" ]; then
      fetch_all
      head=''
    fi
    ;;
  *)
    fetch_all
    since="${INPUT_SINCE:-}"
    ;;
esac

{
  echo "base=$base"
  echo "head=$head"
  echo "since=$since"
} >> "$GITHUB_OUTPUT"
