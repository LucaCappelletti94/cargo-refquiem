#!/usr/bin/env bash
# Posts report findings together in one pull request review.
# MODE selects review, prepare or post, and DRY_RUN renders without posting.
# RUN_HEAD_SHA binds a prepared review to the preparing run's checkout.
# No `set -e`, a failed api call records its own outcome instead of aborting the run.
set -uo pipefail

repo="$GITHUB_REPOSITORY"
pr="${PR_NUMBER:-}"
comments="$PAYLOAD/comments.ndjson"
meta="$PAYLOAD/review.json"
mkdir -p "$PAYLOAD"

finish() {
  echo "$1" > "$STATUS"
  exit "${2:-0}"
}

# Matches the HTTP status `$2` in the gh error output `$1`.
http_status() { grep -qE "HTTP $2|\"status\": \"$2\"" <<< "$1"; }

# Every review body ends with it, so the action knows its own reviews whatever token posted them.
marker='<!-- refquiem -->'

# Dismisses this action's earlier reviews that still request changes on the pull request.
dismiss() {
  local ids id err
  ids="$(gh api "repos/$repo/pulls/$pr/reviews" --paginate --jq '.[]' 2>/dev/null \
    | jq -r --arg marker "$marker" --arg keep "${1:-}" \
      'select(.state == "CHANGES_REQUESTED" and (.body | contains($marker))
        and (.id | tostring) != $keep) | .id')"
  for id in $ids; do
    if ! err="$(gh api -X PUT "repos/$repo/pulls/$pr/reviews/$id/dismissals" -f event=DISMISS \
      -f "message=Dismissed by refquiem in $GITHUB_SERVER_URL/$repo/actions/runs/${GITHUB_RUN_ID:-}" 2>&1)"; then
      printf 'refquiem: review %s stays, its dismissal failed\n%s\n' "$id" "$err"
    fi
  done
}

# The jq definitions every rendering shares, reading $server, $repo, $sha, $reviewed,
# $review_url and $version.
defs='
def blob($commit; $path; $line):
  "\($server)/\($repo)/blob/\($commit)/\($path | split("/") | map(@uri) | join("/"))#L\($line)";
def link($path; $line): blob($sha; $path; $line);
def at: "\(.path):\(.line):\(.column)";
def linked: "- [`\(at)`](\(.link))";
def places: if length == 1
  then "is still mentioned in [`\(.[0] | at)`](\(.[0].link))."
  else "is still mentioned in these places.\n\n" + (map(linked) | join("\n")) end;
def footer($mention):
  "[👎 Report a wrong review](https://github.com/LucaCappelletti94/cargo-refquiem/issues/new?template=wrong-review.yml&title=\("Wrong review on \($reviewed)" | @uri)&version=\($version | @uri)&review=\($review_url | @uri)&mention=\($mention | @uri)) [❤️ Support refquiem](https://github.com/sponsors/LucaCappelletti94)";
def plural($n; $one; $many): "\($n) \(if $n == 1 then $one else $many end)";
'

# Deleted lines accept LEFT comments, added and context lines accept RIGHT comments.
commentable_program='
reduce .[] as $file ({};
  .[$file.filename] = (
    reduce (($file.patch // "") | split("\n")[]) as $line
      ({left: null, right: null, LEFT: [], RIGHT: []};
        if $line | startswith("@@ ") then
          ($line | capture("^@@ -(?<left>[0-9]+)(?:,[0-9]+)? \\+(?<right>[0-9]+)(?:,[0-9]+)? @@")) as $h
          | .left = ($h.left | tonumber) | .right = ($h.right | tonumber)
        elif .left == null then .
        elif $line | startswith("-") then
          .LEFT += [.left] | .left += 1
        elif $line | startswith("+") then
          .RIGHT += [.right] | .right += 1
        elif $line | startswith(" ") then
          .RIGHT += [.right] | .left += 1 | .right += 1
        else . end)
    | {LEFT, RIGHT}))'

# Reads the report into the plan: the groups and markers with links to the head commit, the
# skipped dependencies, the removal comments and the inline comments.
plan_program="$defs"'
def fence($line): ([$line | scan("`+") | length] | max // 0) as $run
  | "`" * (if $run < 3 then 3 else $run + 1 end);
# The headline of a group with the removed name linked to its line in $base and the suggested
# name to its line at head, each where the report places it.
def headlined($base):
  "`\(.path)`" as $name
  | .headline as $h
  | (if .declared_at != null and $base != null and ($h | startswith($name))
      then "[\($name)](\(blob($base; .declared_at.file; .declared_at.line)))\($h | ltrimstr($name))"
      else $h end) as $h
  | if .suggestion != null and .suggested_at != null
    then "`\(.suggestion)`" as $s
      | "[\($s)](\(link(.suggested_at.file; .suggested_at.line)))" as $l
      | $h | split($s) | join($l)
    else $h end;
. as $report
| [$report.groups | to_entries[] | .key as $id | .value | {
    id: $id,
    headline: headlined($report.base),
    declared_at,
    category: (if .suggestion != null then "suggested"
      elif (.candidates | length) > 0 then "candidates" else "none" end),
    mentions: [.mentions[] | {path: .file, line, column, link: link(.file; .line)}]
  }] as $groups
| {
  groups: $groups,
  markers: [$report.unused_markers[] | {path: .file, line, column, link: link(.file; .line),
    text: "This `refquiem: allow(\(.name))` marker suppresses nothing."}],
  skipped: [$report.skipped[].dependency],
  removals: [$groups[] | select(.declared_at != null) | {
    kind: "removal",
    group: .id,
    path: .declared_at.file,
    line: .declared_at.line,
    side: "LEFT",
    at: "\(.declared_at.file):\(.declared_at.line)",
    text: .headline,
    body: ("\(.headline), \(.mentions | places)"
      + "\n\n" + (reduce .mentions[].link as $link ([];
          if index($link) then . else . + [$link] end) | .[:3] | join("\n"))
      + "\n\n" + footer("\(.declared_at.file):\(.declared_at.line)"))
  }],
  inline: ([($report.groups | to_entries[] | .key as $id | (.value | headlined($report.base)) as $h
        | .value.mentions[]
        | {groups: [$id], path: .file, line, column,
           text: "\($h), is still mentioned here.", replacement}),
      ($report.unused_markers[]
        | {groups: [], path: .file, line, column,
           text: "This `refquiem: allow(\(.name))` marker suppresses nothing.",
           replacement: null})]
    | group_by([.path, .line])
    | map(if any(.[]; .replacement != null)
      then [min_by(.column) + {
        text: (map(.text) | unique | join("\n\n")),
        replacement: (map(.replacement | values) | first),
        groups: (map(.groups[]) | unique)}]
      else . end)
    | flatten
    | map(. + {kind: "inline", side: "RIGHT", at: at}
      | . + {body: (.text
        + (if .replacement != null
          then fence(.replacement) as $f | "\n\n\($f)suggestion\n\(.replacement)\n\($f)"
          else "" end)
        + "\n\n" + footer(.at))}))
}'

# Renders the review body from the plan in "$meta", given the groups whose removal comment the
# review holds and the `path:line` of every inline comment it holds. A lone finding its comments
# hold already carries the links, so its body is the marker alone.
review_body() {
  jq -r --arg server "$GITHUB_SERVER_URL" --arg repo "$repo" --arg sha "$head_sha" \
    --arg reviewed "$reviewed" --arg review_url "$review_url" --arg version "$version" \
    --argjson anchored "$1" --argjson posted "$2" \
    --arg run "$GITHUB_SERVER_URL/$repo/actions/runs/${GITHUB_RUN_ID:-}" --arg marker "$marker" \
    "$defs"'
.plan as $p
| ($p.groups | length) as $names
| ([$p.groups[].mentions[]] | length) as $mentions
| ($p.markers | length) as $markers
| def unposted: select(("\(.path):\(.line)") as $key | $posted | index($key) | not);
[ $p.groups[] | select(.id as $id | $anchored | index($id) | not)
  | .headline as $h | [.mentions[] | unposted]
  | select(length > 0)
  | "\($h), \(places)" ]
+ [ [$p.markers[] | unposted | "\(linked) \(.text)"] | select(length > 0) | join("\n") ]
| . as $unplaced
| if $names + $markers == 1 and ($unplaced | length) == 0 and ($p.skipped | length) == 0
  then $marker
  else
  "refquiem found "
  + (if $names > 0 then plural($names; "removed name"; "removed names") + " still mentioned "
      + plural($mentions; "time"; "times") else "" end)
  + (if $names > 0 and $markers > 0 then " and " else "" end)
  + (if $markers > 0 then plural($markers; "suppression marker that suppresses nothing";
      "suppression markers that suppress nothing") else "" end)
  + "."
  + ([["suggested", "with a suggested replacement"],
      ["candidates", "with several possible replacements"],
      ["none", "with no replacement found"]]
    | map(.[0] as $c | ($p.groups | map(select(.category == $c)) | length) as $n
      | select($n > 0) | "- \($n) \(.[1])")
    | if length > 0 then "\n\n" + join("\n") else "" end)
  + (if ($unplaced | length) > 0
    then "\n\nThese are not on a line of this pull request diff, so no comment holds them.\n\n"
      + ($unplaced | join("\n\n"))
    else "" end)
  + (if ($p.skipped | length) > 0
    then "\n\nThese removed dependencies could not be resolved, so their items went unchecked.\n\n"
      + ($p.skipped | map("- `\(.)`") | join("\n"))
    else "" end)
  + "\n\n" + footer("")
  + "\n\n[Run](\($run))\n\n\($marker)"
  end' "$meta"
}

# Tracks findings held by the planned comments.
held() {
  anchored="$(jq -s '[.[] | select(.kind == "removal") | .group]' "$comments")"
  posted="$(jq -s '[.[] | select(.kind == "inline") | "\(.path):\(.line)"]' "$comments")"
}

# Renders every planned comment and the review body into the job summary, posting nothing.
render_dry_run() {
  local rendered='' row
  while IFS= read -r row; do
    [ -n "$row" ] || continue
    rendered+="#### \`$(jq -r .at <<< "$row")\`"
    [ "$(jq -r .side <<< "$row")" = LEFT ] && rendered+=' deleted line'
    rendered+=$'\n\n'"$(jq -r .body <<< "$row")"$'\n\n'
  done < "$comments"
  held
  rendered+=$'#### Review body\n\n'"$(review_body "$anchored" "$posted")"$'\n'
  {
    echo
    echo '### refquiem review, dry run'
    echo
    echo 'The review refquiem would post.'
    echo
    printf '%s' "$rendered"
  } >> "$GITHUB_STEP_SUMMARY"
  printf '%s' "$rendered"
  finish dry-run
}

if [ "$MODE" = post ]; then
  if [ ! -s "$meta" ] || [ ! -f "$comments" ]; then
    echo "refquiem: the prepared review is missing from $PAYLOAD"
    finish failed 1
  fi
  pr="$(jq -r .pr "$meta")"
  head_sha="$(jq -r .head_sha "$meta")"
  version="$(jq -r '.version // ""' "$meta")"
  if ! [[ "$pr" =~ ^[0-9]+$ ]]; then
    echo "refquiem: the prepared review names no pull request"
    finish failed 1
  fi
  reviewed="$repo#$pr"
  review_url="$GITHUB_SERVER_URL/$repo/pull/$pr"
  # Anyone can trigger the preparing run, so the payload posts only on a pull request whose
  # head is still the commit that run checked out.
  if [ -n "${RUN_HEAD_SHA:-}" ]; then
    current="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>/dev/null)" || current=''
    if [ "$head_sha" != "$RUN_HEAD_SHA" ] || [ "$current" != "$RUN_HEAD_SHA" ]; then
      echo "refquiem: pull request $pr is not at $RUN_HEAD_SHA, the review is not posted"
      finish skipped-stale
    fi
  fi
  if [ "$(jq -r '.clean // false' "$meta")" = true ]; then
    echo "refquiem: pull request $pr has no stale mention"
    if [ "$DRY_RUN" != true ]; then
      dismiss
    fi
    finish skipped-clean
  fi
  if [ "$DRY_RUN" = true ]; then
    render_dry_run
  fi
else
  version="${VERSION:-}"
  if [ "$(jq '(.groups | length) + (.unused_markers | length)' "$REPORT")" = 0 ]; then
    echo "refquiem: no stale mention, the review is not posted"
    if [ "$DRY_RUN" != true ] && [ -n "$pr" ]; then
      if [ "$MODE" = prepare ]; then
        if ! head_sha="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
          echo "refquiem: cannot read pull request $pr"
          printf '%s\n' "$head_sha"
          finish failed 1
        fi
        : > "$comments"
        jq -n --arg pr "$pr" --arg head "$head_sha" \
          '{pr: $pr, head_sha: $head, clean: true}' > "$meta"
        echo "refquiem: the clean outcome is prepared in $PAYLOAD for a later job"
        finish prepared
      fi
      dismiss
    fi
    finish skipped-clean
  fi

  head_sha="${GITHUB_SHA:-HEAD}"
  lines='{}'
  if [ "$DRY_RUN" != true ]; then
    if ! head_sha="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
      echo "refquiem: cannot read pull request $pr, the review is not posted"
      printf '%s\n' "$head_sha"
      finish failed 1
    fi
    if ! lines="$(gh api "repos/$repo/pulls/$pr/files" --paginate --jq '.[] | {filename, patch}' \
      | jq -s -c "$commentable_program")"; then
      echo "refquiem: cannot read the files of pull request $pr, the review is not posted"
      finish failed 1
    fi
  fi
  # Without a pull request, a dry run on a push, the report names the run.
  if [ -n "$pr" ]; then
    reviewed="$repo#$pr"
    review_url="$GITHUB_SERVER_URL/$repo/pull/$pr"
  else
    reviewed="$repo"
    review_url="$GITHUB_SERVER_URL/$repo/actions/runs/${GITHUB_RUN_ID:-}"
  fi

  plan="$(jq -c --arg server "$GITHUB_SERVER_URL" --arg repo "$repo" --arg sha "$head_sha" \
    --arg reviewed "$reviewed" --arg review_url "$review_url" --arg version "$version" \
    "$plan_program" "$REPORT")"
  # GitHub rejects a whole review holding a comment off the diff, a dry run shows them all.
  jq -c --argjson lines "$lines" --arg dry "$DRY_RUN" \
    '.removals[], .inline[]
    | select($dry == "true" or (.line as $line | ($lines[.path][.side] // []) | any(.[]; . == $line)))' \
    <<< "$plan" > "$comments"
  jq -n --arg pr "$pr" --arg head "$head_sha" --arg version "$version" --argjson plan "$plan" \
    '{pr: $pr, head_sha: $head, version: $version, plan: ($plan | {groups, markers, skipped})}' \
    > "$meta"

  if [ "$DRY_RUN" = true ]; then
    render_dry_run
  fi
  if [ "$MODE" = prepare ]; then
    echo "refquiem: the review is prepared in $PAYLOAD for a later job"
    finish prepared
  fi
fi

readonly_notice() {
  echo "refquiem: the token is read-only for this pull request, the review is not posted"
  echo "refquiem: the annotations and the job summary carry the findings"
}

# Leaves the review response in `response`.
post_review() {
  response="$(jq -n -c --arg commit "$head_sha" --arg body "$1" --argjson comments "$2" \
    '{commit_id: $commit, event: "REQUEST_CHANGES", body: $body, comments: $comments}' \
    | gh api -X POST "repos/$repo/pulls/$pr/reviews" --input - 2>&1)"
}

# Keep the previous change request until the replacement lands.
held
body="$(review_body "$anchored" "$posted")"
post_review "$body" "$(jq -s -c 'map({path, line, side, body})' "$comments")"
code=$?
if [ "$code" != 0 ] && http_status "$response" 422; then
  echo "refquiem: GitHub rejected a comment, the review is posted with every finding in its body"
  printf '%s\n' "$response"
  body="$(review_body '[]' '[]')"
  post_review "$body" '[]'
  code=$?
fi
{
  echo
  echo '### refquiem review'
  echo
  printf '%s\n' "$body"
} >> "$GITHUB_STEP_SUMMARY"

if [ "$code" != 0 ]; then
  if http_status "$response" 403; then
    readonly_notice
    finish skipped-readonly
  fi
  echo "refquiem: posting the review failed"
  printf '%s\n' "$response"
  finish failed 1
fi
if [ "$(jq -r .state <<< "$response" 2>/dev/null)" != CHANGES_REQUESTED ]; then
  echo "refquiem: the review response is unexpected"
  printf '%s\n' "$response"
  finish failed 1
fi
dismiss "$(jq -r .id <<< "$response")"
echo "refquiem: posted the review on pull request $pr"
finish posted
