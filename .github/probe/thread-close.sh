#!/usr/bin/env bash
# Posts a marked change request with one inline comment, runs the review script on a clean
# report, and checks GitHub left that comment's thread as $1 says, resolved or hidden.
set -euo pipefail
expect="$1"
repo="$GITHUB_REPOSITORY"
pr="$PR_NUMBER"

review="$(jq -n --arg commit "$HEAD_SHA" --arg job "$GITHUB_JOB" \
  '{commit_id: $commit, event: "REQUEST_CHANGES", body: "probe \($job)\n\n<!-- refquiem -->",
    comments: [{path: ".github/probe/thread-close.sh", line: 1, side: "RIGHT", body: "probe \($job)"}]}' \
  | gh api -X POST "repos/$repo/pulls/$pr/reviews" --input -)"
review_id="$(jq -r .id <<< "$review")"
comment="$(gh api "repos/$repo/pulls/$pr/reviews/$review_id/comments" --jq '.[0].node_id')"
echo "probe: review $review_id opened comment $comment"

work="$(mktemp -d)"
echo '{"base": null, "groups": [], "unused_markers": [], "skipped": []}' > "$work/report.json"
MODE=review DRY_RUN=false PAYLOAD="$work/payload" REPORT="$work/report.json" STATUS="$work/status" \
  VERSION=probe bash .github/action/review.sh
echo "probe: review.sh ended $(cat "$work/status")"

threads='query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) { pullRequest(number: $number) {
    reviewThreads(first: 100, after: $endCursor) {
      pageInfo { hasNextPage endCursor }
      nodes { isResolved comments(first: 100) { nodes { id } } }
    } } } }'
resolved="$(gh api graphql --paginate -f query="$threads" -f owner="${repo%%/*}" -f name="${repo#*/}" \
  -F number="$pr" --jq ".data.repository.pullRequest.reviewThreads.nodes[]
    | select(any(.comments.nodes[]; .id == \"$comment\")) | .isResolved")"
hidden="$(gh api graphql -f query='query($id: ID!) { node(id: $id) {
  ... on PullRequestReviewComment { isMinimized minimizedReason } } }' -f id="$comment" --jq '.data.node')"
echo "probe: thread resolved $resolved, comment $hidden"

case "$expect" in
  resolved) [ "$resolved" = true ] ;;
  hidden) [ "$resolved" = false ] \
    && jq -e '.isMinimized and (.minimizedReason | ascii_downcase) == "outdated"' <<< "$hidden" > /dev/null ;;
esac || { echo "probe: expected the thread $expect"; exit 1; }
echo "probe: the thread is $expect"
