#!/usr/bin/env bash
# Posts report findings together in one pull request review.
# MODE review renders and posts, prepare leaves the report for a later job, and post renders
# that report on the pull request its run built. DRY_RUN renders without posting.
# No `set -e`, a failed api call records its own outcome instead of aborting the run.
set -uo pipefail

repo="$GITHUB_REPOSITORY"
pr="${PR_NUMBER:-}"
mkdir -p "$PAYLOAD"
work="$(mktemp -d)"
comments="$work/comments.ndjson"
meta="$work/review.json"
report="$work/report.json"

finish() {
  echo "$1" > "$STATUS"
  exit "${2:-0}"
}

reject() {
  echo "refquiem: $1, nothing is posted"
  finish rejected 1
}

stale() {
  echo "refquiem: $1, nothing is posted"
  finish skipped-stale
}

uri() { jq -rn --arg s "$1" '$s | @uri'; }

# Whether file $1 holds at most $2 bytes.
within() { [ "$(wc -c < "$1" | tr -d ' ')" -le "$2" ]; }

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

# Every review thread with its id, state, comments and the review its first comment opened.
threads_query='query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(first: 100, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes { id isResolved comments(first: 100) { nodes { id pullRequestReview { databaseId body } } } }
    } } } }'
resolve_mutation='mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }'
hide_mutation='mutation($id: ID!) { minimizeComment(input: {subjectId: $id, classifier: OUTDATED}) { minimizedComment { isMinimized } } }'

# Hides every comment of the thread `$1` holds as outdated.
hide_thread() {
  local comment err
  for comment in $(jq -r '.comments.nodes[].id' <<< "$1"); do
    if ! err="$(gh api graphql -f query="$hide_mutation" -f id="$comment" 2>&1)"; then
      printf 'refquiem: comment %s stays visible, hiding it failed\n%s\n' "$comment" "$err"
    fi
  done
}

# Closes the open threads of this action's earlier reviews, keeping those of review $1. A thread
# resolves when the token may resolve it, which needs contents: write, and is hidden otherwise.
close_threads() {
  local threads row thread err hiding=false
  if ! threads="$(gh api graphql --paginate -f query="$threads_query" -f owner="${repo%%/*}" \
    -f name="${repo#*/}" -F number="$pr" --jq '.data.repository.pullRequest.reviewThreads.nodes[]' \
    2> "$work/threads.err")"; then
    printf 'refquiem: the review threads stay open, they cannot be read\n%s\n' "$(cat "$work/threads.err")"
    return
  fi
  while IFS= read -r row; do
    [ -n "$row" ] || continue
    thread="$(jq -r .id <<< "$row")"
    if [ "$hiding" = false ]; then
      if err="$(gh api graphql -f query="$resolve_mutation" -f id="$thread" 2>&1)"; then
        continue
      fi
      if ! grep -q 'Resource not accessible' <<< "$err"; then
        printf 'refquiem: thread %s stays open, resolving it failed\n%s\n' "$thread" "$err"
        continue
      fi
      echo "refquiem: the token may not resolve threads, their comments are hidden as outdated"
      hiding=true
    fi
    hide_thread "$row"
  done < <(jq -c --arg marker "$marker" --arg keep "${1:-}" \
    'select(.isResolved | not)
      | (.comments.nodes[0].pullRequestReview // {}) as $review
      | select(($review.body // "" | contains($marker)) and ($review.databaseId | tostring) != $keep)' \
    <<< "$threads")
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
# A code span holding any text, its fence longer than every backtick run inside.
def code: ([scan("`+") | length] | max // 0) as $run
  | ("`" * ($run + 1)) as $f
  | if $run > 0 then "\($f) \(.) \($f)" else "\($f)\(.)\($f)" end;
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

# Keeps the report fields the review renders. Names render in code spans, so they hold no
# backtick or control character, and each headline is the one its group's fields produce.
canonical_program='
def str: type == "string";
def int: type == "number" and . >= 1 and . <= 2147483647 and . == floor;
def name: str and length > 0 and (test("[`\\x00-\\x1f\\x7f]") | not);
def line: str and (test("[\\x00-\\x08\\x0a-\\x1f\\x7f]") | not);
def commit: str and test("^[0-9a-f]{40}([0-9a-f]{24})?$");
def need(c): if c then . else error("malformed") end;
def position: if . == null then null
  else need(type == "object" and (.file | name) and (.line | int)) | {file, line} end;
def kinds: {module: "module", struct: "struct", enum: "enum", union: "union", trait: "trait",
  trait_alias: "trait alias", type_alias: "type alias", const: "const", static: "static",
  function: "function", macro: "macro", variant: "variant", field: "field", method: "method",
  associated_const: "associated const", associated_type: "associated type",
  reexport: "re-export", package: "package", dependency: "dependency", feature: "feature",
  binary: "binary", example: "example", serialized: "serialized name"};
def headline($change):
  . as $g
  | (if any(("package", "dependency", "feature", "binary", "example"); . == $g.kind)
      then "removed" else "deleted" end) as $verb
  | "`\($g.path)` (\(kinds[$g.kind])"
    + (if $g.member != null then " of `\($g.member)`" else "" end) + ")"
    + (if $g.removed_in == null then ""
      elif $g.removed_in == "working_tree" then ", \($verb) in the working tree"
      else ", \($verb) in \($g.removed_in[:7])" end)
    + (if $g.suggestion != null then ", \($change) `\($g.suggestion)`"
      elif ($g.candidates | length) > 0
      then ", perhaps \($g.candidates[:-1] | map("`\(.)`") | join(", ")) or `\($g.candidates[-1])`"
      else "" end);
def group:
  need(type == "object" and (.path | name) and (.kind | str) and kinds[.kind] != null
    and (.member == null or (.member | name))
    and (.removed_in == null or .removed_in == "working_tree" or (.removed_in | commit))
    and (.suggestion == null or (.suggestion | name))
    and (.candidates | type == "array" and all(.[]; name))
    and (.headline | str) and (.mentions | type == "array"))
  | need(. as $g | if $g.suggestion != null
      then any(("moved to", "renamed to", "replaced by"); . as $c | $g | headline($c) == $g.headline)
      else headline(null) == $g.headline end)
  | {path, kind, member, removed_in, declared_at: (.declared_at | position), suggestion,
     suggested_at: (if .suggestion == null then null else .suggested_at | position end),
     candidates, headline,
     mentions: [.mentions[]
       | need(type == "object" and (.file | name) and (.line | int) and (.column | int)
         and (.text | str) and (.replacement == null or (.replacement | line))
         and (.uppercase == null or (.uppercase | type == "object"
           and (.spelling | name) and (.vocabulary | line) and (.vocabulary | length > 0))))
       | {file, line, column, text, replacement,
          uppercase: (if .uppercase == null then null else .uppercase | {spelling, vocabulary} end)}]};
.[0]
| need(type == "object" and (.groups | type == "array") and (.unused_markers | type == "array")
  and (.skipped | type == "array") and (.base == null or (.base | commit)))
| {base,
   groups: [.groups[] | group],
   unused_markers: [.unused_markers[]
     | need(type == "object" and (.file | name) and (.line | int) and (.column | int)
       and (.name | name))
     | {file, line, column, name}],
   skipped: [.skipped[] | need(type == "object" and (.dependency | name)) | {dependency}]}'

# Validates the report in $1 into "$report".
canonical() { jq -c -s "$canonical_program" "$1" > "$report" 2>/dev/null; }

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
           text: ("\($h), is still mentioned here."
             + (if .uppercase == null then ""
               else " Write \(.uppercase.spelling | code) to mark it as \(.uppercase.vocabulary | code)." end)),
           replacement}),
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

# The pull request the payload names, accepted only when run $RUN_ID built it at the payload's
# head. Everything but the run id comes from the API.
bind_to_run() {
  local run run_event run_head run_repo run_owner run_branch open built
  [[ "${RUN_ID:-}" =~ ^[0-9]+$ ]] || reject "the triggering run has no id"
  if ! run="$(gh api "repos/$repo/actions/runs/$RUN_ID" 2>&1)"; then
    echo "refquiem: cannot read run $RUN_ID, the review is not posted"
    printf '%s\n' "$run"
    finish failed 1
  fi
  run_event="$(jq -r '.event // empty' <<< "$run")"
  run_head="$(jq -r '.head_sha // empty' <<< "$run")"
  run_repo="$(jq -r '.head_repository.full_name // empty' <<< "$run")"
  run_owner="$(jq -r '.head_repository.owner.login // empty' <<< "$run")"
  run_branch="$(jq -r '.head_branch // empty' <<< "$run")"
  [ "$run_event" = pull_request ] || reject "the triggering run did not come from a pull request"
  [[ "$run_head" =~ ^[0-9a-f]{40}$ ]] && [ -n "$run_repo" ] && [ -n "$run_owner" ] \
    && [ -n "$run_branch" ] || reject "the triggering run names no head"
  if ! open="$(gh api "repos/$repo/pulls?state=open&head=$(uri "$run_owner:$run_branch")&per_page=100" \
    --paginate --jq '.[]' 2>&1)"; then
    echo "refquiem: cannot list the open pull requests, the review is not posted"
    printf '%s\n' "$open"
    finish failed 1
  fi
  built="$(jq -r --arg sha "$run_head" --arg full "$run_repo" \
    'select(.head.sha == $sha and .head.repo.full_name == $full) | .number' <<< "$open" 2>/dev/null)"
  [ -n "$built" ] || stale "no open pull request has the head run $RUN_ID built"
  grep -qx -- "$pr" <<< "$built" || reject "run $RUN_ID did not build pull request $pr"
  [ "$head_sha" = "$run_head" ] || reject "the prepared head is not the head run $RUN_ID built"
}

case "$MODE" in
  prepare)
    version="${VERSION:-}"
    if [ "$DRY_RUN" != true ]; then
      [[ "$pr" =~ ^[1-9][0-9]*$ ]] || { echo "refquiem: prepare needs pr-number"; finish failed 1; }
      head_sha="${SCANNED_HEAD:-}"
      if [ -z "$head_sha" ] && ! head_sha="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
        echo "refquiem: cannot read pull request $pr"
        printf '%s\n' "$head_sha"
        finish failed 1
      fi
      cp "$REPORT" "$PAYLOAD/report.json"
      jq -n --arg pr "$pr" --arg head "$head_sha" --arg version "$version" \
        '{pr: $pr, head_sha: $head, version: $version}' > "$PAYLOAD/meta.json"
      echo "refquiem: the report is prepared in $PAYLOAD for a later job"
      finish prepared
    fi
    canonical "$REPORT" || reject "the report is malformed"
    ;;
  post)
    for f in meta.json report.json; do
      if [ ! -f "$PAYLOAD/$f" ]; then
        echo "refquiem: the prepared review in $PAYLOAD lacks $f"
        finish failed 1
      fi
    done
    within "$PAYLOAD/meta.json" 4096 && within "$PAYLOAD/report.json" 16777216 \
      || reject "the prepared review is too large"
    jq -e -s '.[0] | type == "object" and (.pr | type == "string" and test("^[1-9][0-9]{0,9}$"))
      and (.head_sha | type == "string" and test("^[0-9a-f]{40}$"))
      and ((.version // "") | type == "string" and test("^([0-9A-Za-z][0-9A-Za-z.+-]{0,39})?$"))' \
      "$PAYLOAD/meta.json" > /dev/null 2>&1 || reject "the prepared metadata is malformed"
    claimed="$(jq -r '.pr' "$PAYLOAD/meta.json")"
    [ -z "$pr" ] || [ "$pr" = "$claimed" ] || reject "the job names pull request $pr, the payload $claimed"
    pr="$claimed"
    head_sha="$(jq -r '.head_sha' "$PAYLOAD/meta.json")"
    version="$(jq -r '.version // ""' "$PAYLOAD/meta.json")"
    if [ "${EVENT_NAME:-}" = workflow_run ]; then
      bind_to_run
    else
      # No run vouches for the payload here, so the job names the pull request itself.
      [ -n "${PR_NUMBER:-}" ] || reject "outside workflow_run the job names the pull request"
      if ! current="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
        echo "refquiem: cannot read pull request $pr, the review is not posted"
        printf '%s\n' "$current"
        finish failed 1
      fi
      [ "$current" = "$head_sha" ] || stale "pull request $pr moved on from the prepared head"
    fi
    canonical "$PAYLOAD/report.json" || reject "the prepared report is malformed"
    ;;
  *)
    version="${VERSION:-}"
    canonical "$REPORT" || reject "the report is malformed"
    ;;
esac

if [ "$(jq '(.groups | length) + (.unused_markers | length)' "$report")" = 0 ]; then
  echo "refquiem: no stale mention, the review is not posted"
  if [ "$DRY_RUN" != true ] && [ -n "$pr" ]; then
    dismiss
    close_threads
  fi
  finish skipped-clean
fi

lines='{}'
if [ "$MODE" != post ]; then
  head_sha="${GITHUB_SHA:-HEAD}"
fi
if [ "$DRY_RUN" != true ]; then
  if [ "$MODE" != post ] && ! head_sha="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
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
  "$plan_program" "$report")"
# GitHub rejects a whole review holding a comment off the diff, a dry run shows them all.
jq -c --argjson lines "$lines" --arg dry "$DRY_RUN" \
  '.removals[], .inline[]
  | select($dry == "true" or (.line as $line | ($lines[.path][.side] // []) | any(.[]; . == $line)))' \
  <<< "$plan" > "$comments"
jq -n --argjson plan "$plan" '{plan: ($plan | {groups, markers, skipped})}' > "$meta"

if [ "$DRY_RUN" = true ]; then
  render_dry_run
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
anchors=true
if [ "$code" != 0 ] && http_status "$response" 422; then
  echo "refquiem: GitHub rejected a comment, the review is posted with every finding in its body"
  printf '%s\n' "$response"
  body="$(review_body '[]' '[]')"
  post_review "$body" '[]'
  code=$?
  anchors=false
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
review_id="$(jq -r .id <<< "$response")"
dismiss "$review_id"
# A review anchoring nothing leaves the earlier threads as the only pointers at its findings.
if [ "$anchors" = true ]; then
  close_threads "$review_id"
fi
echo "refquiem: posted the review on pull request $pr"
finish posted
