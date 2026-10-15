#!/usr/bin/env bash
# Turns a `--json` report into a pull request review that requests changes.
# MODE review computes and posts it, prepare writes it to PAYLOAD, post posts a prepared one.
# DRY_RUN renders it into the job summary instead. STATUS receives the outcome.
# RUN_HEAD_SHA, in post mode, is the commit the run that prepared the payload checked out.
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

if [ "$MODE" = post ]; then
  if [ ! -s "$meta" ] || [ ! -f "$comments" ]; then
    echo "refquiem: the prepared review is missing from $PAYLOAD"
    finish failed 1
  fi
  pr="$(jq -r .pr "$meta")"
  head_sha="$(jq -r .head_sha "$meta")"
  out_of_range="$(jq -r .out_of_range "$meta")"
  if ! [[ "$pr" =~ ^[0-9]+$ ]]; then
    echo "refquiem: the prepared review names no pull request"
    finish failed 1
  fi
  # Anyone can trigger the preparing run, so the payload posts only on a pull request whose
  # head is still the commit that run checked out.
  if [ -n "${RUN_HEAD_SHA:-}" ]; then
    current="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>/dev/null)" || current=''
    if [ "$head_sha" != "$RUN_HEAD_SHA" ] || [ "$current" != "$RUN_HEAD_SHA" ]; then
      echo "refquiem: pull request $pr is not at $RUN_HEAD_SHA, the review is not posted"
      finish skipped-stale
    fi
  fi
else
  # One row per mention and per unused marker, the text it says and where.
  rows="$(jq -c '
    (.groups[] | .headline as $h | .mentions[]
      | {path: .file, line, column, text: "\($h), is still mentioned here."}),
    (.unused_markers[]
      | {path: .file, line, column,
         text: "This `refquiem: allow(\(.name))` marker suppresses nothing."})' "$REPORT")"
  if [ -z "$rows" ]; then
    echo "refquiem: no stale mention, the review is not posted"
    finish skipped-clean
  fi

  head_sha=''
  changed='[]'
  if [ "$DRY_RUN" != true ]; then
    if ! head_sha="$(gh api "repos/$repo/pulls/$pr" --jq .head.sha 2>&1)"; then
      echo "refquiem: cannot read pull request $pr, the review is not posted"
      printf '%s\n' "$head_sha"
      finish failed 1
    fi
    changed="$(gh api "repos/$repo/pulls/$pr/files" --paginate --jq '.[].filename' | jq -R . | jq -s .)"
  fi

  uri() { jq -rn --arg s "$1" '$s | @uri'; }
  support_link='[❤️ Support refquiem](https://github.com/sponsors/LucaCappelletti94)'
  # Without a pull request, a dry run on a push, the report names the run.
  if [ -n "$pr" ]; then
    reviewed="$repo#$pr"
    review_url="$GITHUB_SERVER_URL/$repo/pull/$pr"
  else
    reviewed="$repo"
    review_url="$GITHUB_SERVER_URL/$repo/actions/runs/${GITHUB_RUN_ID:-}"
  fi
  footer() {
    printf '[👎 Report a wrong review](https://github.com/LucaCappelletti94/cargo-refquiem/issues/new?template=wrong-review.yml&title=%s&version=%s&review=%s&mention=%s) %s' \
      "$(uri "Wrong review on $reviewed")" \
      "$(uri "${VERSION:-}")" \
      "$(uri "$review_url")" \
      "$(uri "$1")" \
      "$support_link"
  }

  : > "$comments"
  out_of_range=''
  rendered=''
  while IFS= read -r row; do
    path="$(jq -r .path <<< "$row")"
    at="$path:$(jq -r '"\(.line):\(.column)"' <<< "$row")"
    text="$(jq -r .text <<< "$row")"
    # GitHub anchors a comment only in a file the pull request changes, a dry run shows them all.
    if [ "$DRY_RUN" != true ] && [ "$(jq --arg p "$path" 'index($p) != null' <<< "$changed")" != true ]; then
      out_of_range+="- \`$at\` $text"$'\n'
      continue
    fi
    body="$text"$'\n\n'"$(footer "$at")"
    if [ "$DRY_RUN" = true ]; then
      rendered+="#### \`$at\`"$'\n\n'"$body"$'\n\n'
    else
      jq -c --arg body "$body" '. + {body: $body}' <<< "$row" >> "$comments"
    fi
  done <<< "$rows"

  if [ "$DRY_RUN" = true ]; then
    {
      echo
      echo '### refquiem review, dry run'
      echo
      echo 'The comments refquiem would post, as if every file were in the pull request diff.'
      echo
      printf '%s' "$rendered"
    } >> "$GITHUB_STEP_SUMMARY"
    printf '%s' "$rendered"
    finish dry-run
  fi

  jq -n --arg pr "$pr" --arg head "$head_sha" --arg oor "$out_of_range" \
    '{pr: $pr, head_sha: $head, out_of_range: $oor}' > "$meta"
  if [ -n "$out_of_range" ]; then
    {
      echo
      echo '### Mentions outside the pull request diff'
      echo
      printf '%s' "$out_of_range"
    } >> "$GITHUB_STEP_SUMMARY"
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

# The first post is the permission probe.
failed=''
while IFS= read -r row; do
  [ -n "$row" ] || continue
  path="$(jq -r .path <<< "$row")"
  line="$(jq -r .line <<< "$row")"
  at="$path:$line:$(jq -r .column <<< "$row")"
  if ! err="$(jq -r .body <<< "$row" | gh api "repos/$repo/pulls/$pr/comments" \
    -f "commit_id=$head_sha" -f "path=$path" -F "line=$line" -f side=RIGHT -F 'body=@-' 2>&1)"; then
    if http_status "$err" 403; then
      readonly_notice
      finish skipped-readonly
    fi
    printf 'refquiem: %s\n' "$at" "$err"
    # 422 is a line outside the diff hunks, the review body lists it instead.
    if http_status "$err" 422; then
      out_of_range+="- \`$at\` $(jq -r .text <<< "$row")"$'\n'
    else
      failed+="- \`$at\` $(jq -r .text <<< "$row")"$'\n'
    fi
  fi
done < "$comments"

review_body="refquiem found removed names that are still mentioned, or suppression markers that suppress nothing."
if [ -n "$out_of_range" ]; then
  review_body+=$'\n\n'"These are outside the lines the pull request diff shows, so they carry no comment."$'\n\n'"$out_of_range"
fi
if [ -n "$failed" ]; then
  review_body+=$'\n\n'"These comments could not be posted."$'\n\n'"$failed"
fi
review_body+=$'\n\n'"[Run]($GITHUB_SERVER_URL/$repo/actions/runs/${GITHUB_RUN_ID:-})"

if ! response="$(printf '%s' "$review_body" | gh api -X POST "repos/$repo/pulls/$pr/reviews" \
  -f event=REQUEST_CHANGES -F 'body=@-' 2>&1)"; then
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
echo "refquiem: posted the review on pull request $pr"
finish posted
