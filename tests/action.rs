//! The GitHub Action's scripts: the range each event scans and the pull request review.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::Repo;

const ZERO: &str = "0000000000000000000000000000000000000000";

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".github/action")
        .join(name)
}

/// The `key=value` lines a step wrote to `GITHUB_OUTPUT`.
fn outputs(file: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(file)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// A shallow clone of `origin` at `branch`, as `actions/checkout` leaves it.
fn shallow_clone(origin: &Repo, branch: &str) -> tempfile::TempDir {
    origin.git(&["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let clone = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["clone", "-q", "--depth", "1", "--branch", branch])
        .arg(format!("file://{}", origin.path().display()))
        .arg(clone.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success());
    clone
}

/// Runs the range script in `clone` for `event` and returns its outputs.
fn range(
    clone: &Path,
    event: &str,
    payload: &str,
    sha: &str,
    inputs: &[(&str, &str)],
) -> Vec<(String, String)> {
    let dir = tempfile::tempdir().unwrap();
    let event_path = dir.path().join("event.json");
    let output = dir.path().join("output");
    std::fs::write(&event_path, payload).unwrap();
    std::fs::write(&output, "").unwrap();
    let mut command = Command::new("bash");
    command
        .arg(script("range.sh"))
        .current_dir(clone)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GITHUB_EVENT_NAME", event)
        .env("GITHUB_EVENT_PATH", &event_path)
        .env("GITHUB_SHA", sha)
        .env("GITHUB_OUTPUT", &output);
    for (key, value) in inputs {
        command.env(key, value);
    }
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    outputs(&output)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_owned(), value.to_owned())
}

/// The checkout is the merge commit, and the scan ends at the pull request's head so mention
/// lines match the commit review comments anchor to.
#[test]
fn a_pull_request_scans_its_own_commits_up_to_its_head() {
    let origin = Repo::new();
    origin.commit("a");
    let fork = origin.commit("b");
    origin.git(&["checkout", "-q", "-b", "pr"]);
    // More commits than one deepening step fetches.
    for n in 0..100 {
        origin.commit(&n.to_string());
    }
    let head = origin.commit("tip");
    origin.git(&["checkout", "-q", "main"]);
    let base = origin.commit("base moved on");
    origin.git(&["checkout", "-q", "-b", "merge"]);
    origin.git(&["merge", "-q", "--no-ff", "-m", "merge", "pr"]);
    let merge = origin.rev("HEAD");
    let clone = shallow_clone(&origin, "merge");

    let out = range(
        clone.path(),
        "pull_request",
        &format!(
            "{{\"pull_request\": {{\"base\": {{\"sha\": \"{base}\"}}, \"head\": {{\"sha\": \"{head}\"}}}}}}"
        ),
        &merge,
        &[],
    );
    assert_eq!(
        out,
        [pair("base", &fork), pair("head", &head), pair("since", "")]
    );
    assert_eq!(git(clone.path(), &["merge-base", &fork, &head]), fork);
}

#[test]
fn a_push_scans_the_previous_tip_and_the_pushed_commits() {
    let origin = Repo::new();
    let before = origin.commit("a");
    origin.commit("b");
    let after = origin.commit("c");
    let clone = shallow_clone(&origin, "main");

    let out = range(
        clone.path(),
        "push",
        &format!(
            "{{\"before\": \"{before}\", \"after\": \"{after}\", \"repository\": {{\"default_branch\": \"main\"}}}}"
        ),
        &after,
        &[],
    );
    assert_eq!(
        out,
        [
            pair("base", &before),
            pair("head", &after),
            pair("since", "")
        ]
    );
    assert_eq!(git(clone.path(), &["merge-base", &before, &after]), before);
}

#[test]
fn a_new_branch_push_scans_from_its_merge_base_with_the_default_branch() {
    let origin = Repo::new();
    let fork = origin.commit("a");
    origin.commit("b");
    origin.git(&["checkout", "-q", "-b", "feature", &fork]);
    let after = origin.commit("c");
    let clone = shallow_clone(&origin, "feature");

    let out = range(
        clone.path(),
        "push",
        &format!(
            "{{\"before\": \"{ZERO}\", \"after\": \"{after}\", \"repository\": {{\"default_branch\": \"main\"}}}}"
        ),
        &after,
        &[],
    );
    assert_eq!(
        out,
        [pair("base", &fork), pair("head", &after), pair("since", "")]
    );
}

#[test]
fn a_force_push_scans_from_the_merge_base_of_the_old_and_new_tips() {
    let origin = Repo::new();
    let fork = origin.commit("a");
    origin.git(&["checkout", "-q", "-b", "topic"]);
    let before = origin.commit("old");
    origin.git(&["update-ref", "refs/keep/old", &before]);
    origin.git(&["reset", "-q", "--hard", &fork]);
    let after = origin.commit("new");
    let clone = shallow_clone(&origin, "topic");

    let out = range(
        clone.path(),
        "push",
        &format!(
            "{{\"before\": \"{before}\", \"after\": \"{after}\", \"repository\": {{\"default_branch\": \"main\"}}}}"
        ),
        &after,
        &[],
    );
    assert_eq!(
        out,
        [pair("base", &fork), pair("head", &after), pair("since", "")]
    );
}

#[test]
fn a_merge_group_scans_its_base_and_head() {
    let origin = Repo::new();
    let base = origin.commit("a");
    let head = origin.commit("b");
    let clone = shallow_clone(&origin, "main");

    let out = range(
        clone.path(),
        "merge_group",
        &format!("{{\"merge_group\": {{\"base_sha\": \"{base}\", \"head_sha\": \"{head}\"}}}}"),
        &head,
        &[],
    );
    assert_eq!(
        out,
        [pair("base", &base), pair("head", &head), pair("since", "")]
    );
}

/// The trigger runs with the base repository's credentials, so the action refuses it.
#[test]
fn a_pull_request_target_run_is_refused() {
    let origin = Repo::new();
    let head = origin.commit("a");
    let clone = shallow_clone(&origin, "main");
    let dir = tempfile::tempdir().unwrap();
    let event = dir.path().join("event.json");
    let output = dir.path().join("output");
    std::fs::write(
        &event,
        format!(
            r#"{{"pull_request": {{"base": {{"sha": "{head}"}}, "head": {{"sha": "{head}"}}}}}}"#
        ),
    )
    .unwrap();
    std::fs::write(&output, "").unwrap();
    let result = Command::new("bash")
        .arg(script("range.sh"))
        .current_dir(clone.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GITHUB_EVENT_NAME", "pull_request_target")
        .env("GITHUB_EVENT_PATH", &event)
        .env("GITHUB_SHA", &head)
        .env("GITHUB_OUTPUT", &output)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("pull_request_target"),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert_eq!(std::fs::read_to_string(&output).unwrap(), "");
}

#[test]
fn a_manual_audit_fetches_the_whole_history_and_tags_and_keeps_since() {
    let origin = Repo::new();
    let first = origin.commit("a");
    origin.git(&["tag", "v0.1.0"]);
    let head = origin.commit("b");
    let clone = shallow_clone(&origin, "main");

    let out = range(
        clone.path(),
        "workflow_dispatch",
        "{}",
        &head,
        &[("INPUT_SINCE", "v0.1.0")],
    );
    assert_eq!(
        out,
        [pair("base", ""), pair("head", ""), pair("since", "v0.1.0")]
    );
    assert_eq!(
        git(clone.path(), &["rev-parse", "--is-shallow-repository"]),
        "false"
    );
    assert_eq!(git(clone.path(), &["rev-parse", "v0.1.0^{commit}"]), first);
}

#[test]
fn a_push_sharing_no_history_with_the_default_branch_scans_everything() {
    let origin = Repo::new();
    origin.commit("a");
    origin.git(&["checkout", "-q", "--orphan", "pages"]);
    origin.commit("site");
    let after = origin.commit("more");
    let clone = shallow_clone(&origin, "pages");

    let out = range(
        clone.path(),
        "push",
        &format!(
            "{{\"before\": \"{ZERO}\", \"after\": \"{after}\", \"repository\": {{\"default_branch\": \"main\"}}}}"
        ),
        &after,
        &[],
    );
    assert_eq!(out, [pair("base", ""), pair("head", ""), pair("since", "")]);
    assert_eq!(
        git(clone.path(), &["rev-parse", "--is-shallow-repository"]),
        "false"
    );
}

/// A fake `gh` logging every call with its stdin, answering the review's api calls.
struct FakeGh {
    dir: tempfile::TempDir,
}

/// The reviews the pull request already has: one of ours requesting changes, one of ours
/// already dismissed, and a maintainer's request without the marker.
const EARLIER_REVIEWS: &str = r#"{"id":101,"state":"CHANGES_REQUESTED","body":"stale\n\n<!-- refquiem -->"}
{"id":102,"state":"DISMISSED","body":"older\n\n<!-- refquiem -->"}
{"id":103,"state":"CHANGES_REQUESTED","body":"a maintainer's request"}"#;

/// The review threads the pull request already has, as `--jq` prints each node: an open one
/// of ours with a reply, an open one of our dismissed review, a maintainer's open one, and one
/// of ours already resolved.
const EARLIER_THREADS: &str = r#"{"id":"T1","isResolved":false,"comments":{"nodes":[{"id":"C11","pullRequestReview":{"databaseId":101,"body":"stale\n\n<!-- refquiem -->"}},{"id":"C12","pullRequestReview":{"databaseId":201,"body":"fixed in the next commit"}}]}}
{"id":"T2","isResolved":false,"comments":{"nodes":[{"id":"C21","pullRequestReview":{"databaseId":102,"body":"older\n\n<!-- refquiem -->"}}]}}
{"id":"T3","isResolved":false,"comments":{"nodes":[{"id":"C31","pullRequestReview":{"databaseId":103,"body":"a maintainer's request"}}]}}
{"id":"T4","isResolved":true,"comments":{"nodes":[{"id":"C41","pullRequestReview":{"databaseId":101,"body":"stale\n\n<!-- refquiem -->"}}]}}"#;

/// The head commit the fake pull request 7 and the fake run 99 point at.
const HEAD: &str = "f00df00df00df00df00df00df00df00df00df00d";

impl FakeGh {
    /// `changed` are the pull request's files, each a hunk covering its first 200 lines on
    /// both sides. `fail` makes the review post fail with that status for `review`, only a
    /// post carrying comments for `review-comments`, listing the files for `files`, a
    /// dismissal for `dismissals`, reading the run for `runs`, or reading the review threads
    /// for `threads`. `resolve` refuses resolving a thread the way a token without
    /// `contents: write` is refused, and `close` refuses hiding its comments as well. Run 99
    /// built pull request 7 from the `feature` branch of `fork/repo` at [`HEAD`].
    fn new(changed: &[&str], fail: Option<(&str, u16)>) -> Self {
        let whole = format!(
            "@@ -1,200 +1,200 @@\n{}{}",
            "-\n".repeat(200),
            "+\n".repeat(200)
        );
        let files: Vec<(&str, &str)> = changed.iter().map(|path| (*path, whole.as_str())).collect();
        Self::with_patches(&files, fail)
    }

    /// [`FakeGh::new`] with the pull request's files and their diff patches.
    fn with_patches(files: &[(&str, &str)], fail: Option<(&str, u16)>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (fail_key, fail_status) = fail.map_or((String::new(), 0), |(p, s)| (p.to_owned(), s));
        let mut listed = Vec::new();
        for (path, patch) in files {
            serde_json::to_writer(
                &mut listed,
                &serde_json::json!({"filename": path, "patch": patch}),
            )
            .unwrap();
            listed.push(b'\n');
        }
        std::fs::write(dir.path().join("files.ndjson"), listed).unwrap();
        let gh = Self { dir };
        gh.run(&serde_json::json!({"event": "pull_request"}), HEAD);
        gh.open(&[7]);
        let dir = gh.dir;
        let script = format!(
            r#"#!/usr/bin/env bash
log="{log}"
body=''
for arg in "$@"; do
  if [ "$arg" = 'body=@-' ] || [ "$arg" = '-' ]; then body="$(cat)"; fi
done
printf 'CALL %s\n%s\nEND\n' "$*" "$body" >> "$log"
case "$*" in
  *graphql*resolveReviewThread*)
    if [ '{fail_key}' = resolve ] || [ '{fail_key}' = close ]; then
      echo 'gh: GraphQL: Resource not accessible by integration (resolveReviewThread)' >&2
      exit 1
    fi
    echo '{{"data":{{"resolveReviewThread":{{"thread":{{"isResolved":true}}}}}}}}' ;;
  *graphql*minimizeComment*)
    if [ '{fail_key}' = close ]; then
      echo 'gh: GraphQL: Resource not accessible by integration (minimizeComment)' >&2
      exit 1
    fi
    echo '{{"data":{{"minimizeComment":{{"minimizedComment":{{"isMinimized":true}}}}}}}}' ;;
  *graphql*reviewThreads*)
    if [ '{fail_key}' = threads ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    cat <<'THREADS'
{threads}
THREADS
    if grep -q '^CALL .*-X POST.*pulls/7/reviews' "$log"; then
      echo '{{"id":"T5","isResolved":false,"comments":{{"nodes":[{{"id":"C51","pullRequestReview":{{"databaseId":104,"body":"new\n\n<!-- refquiem -->"}}}}]}}}}'
    fi
    ;;
  *pulls/7/reviews/*/dismissals*)
    if [ '{fail_key}' = dismissals ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    echo '{{"state": "DISMISSED"}}' ;;
  *'-X POST'*pulls/7/reviews*)
    if [ '{fail_key}' = review ] || {{ [ '{fail_key}' = review-comments ] && [[ "$body" != *'"comments":[]'* ]]; }}; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    echo '{{"id": 104, "state": "CHANGES_REQUESTED"}}' ;;
  *pulls/7/reviews*)
    cat <<'REVIEWS'
{reviews}
REVIEWS
    if grep -q '^CALL .*-X POST.*pulls/7/reviews' "$log"; then
      echo '{{"id":104,"state":"CHANGES_REQUESTED","body":"new\n\n<!-- refquiem -->"}}'
    fi
    ;;
  *pulls/7/files*)
    if [ '{fail_key}' = files ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    cat "{files}" ;;
  *actions/runs/*)
    if [ '{fail_key}' = runs ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    cat "{run}" ;;
  *'pulls?state=open'*) cat "{open}" ;;
  *pulls/7*) echo 'f00df00df00df00df00df00df00df00df00df00d' ;;
  *) echo "unexpected gh $*" >&2; exit 1 ;;
esac
"#,
            log = dir.path().join("log").display(),
            files = dir.path().join("files.ndjson").display(),
            run = dir.path().join("run.json").display(),
            open = dir.path().join("open.ndjson").display(),
            reviews = EARLIER_REVIEWS,
            threads = EARLIER_THREADS,
        );
        let gh = dir.path().join("gh");
        std::fs::write(&gh, script).unwrap();
        Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
        std::fs::write(dir.path().join("log"), "").unwrap();
        Self { dir }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("log")).unwrap()
    }

    /// Makes run 99 the run `fields` describe, built from `fork/repo` at `head`.
    fn run(&self, fields: &serde_json::Value, head: &str) {
        let mut run = serde_json::json!({
            "head_sha": head,
            "head_branch": "feature",
            "head_repository": {"full_name": "fork/repo", "owner": {"login": "fork"}},
        });
        run.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        std::fs::write(self.dir.path().join("run.json"), run.to_string()).unwrap();
    }

    /// Makes `numbers` the open pull requests whose head is `fork:feature` at [`HEAD`].
    fn open(&self, numbers: &[u64]) {
        let open: String = numbers
            .iter()
            .map(|number| {
                serde_json::json!({"number": number, "head": {"sha": HEAD, "repo": {"full_name": "fork/repo"}}})
                    .to_string()
                    + "\n"
            })
            .collect();
        std::fs::write(self.dir.path().join("open.ndjson"), open).unwrap();
    }
}

/// The `--json --no-fail` scan of a repository whose `fixture::gone` is mentioned at
/// `README.md:3:7` and `docs/old.md:9:2`, with an unused marker at `src/lib.rs:1:20`.
fn stale_scan() -> String {
    scan(&stale_repo(), &["--since", "HEAD~1"])
}

/// [`stale_scan`] over the full history, which places no declaration.
fn stale_scan_of_the_full_history() -> String {
    scan(&stale_repo(), &[])
}

fn stale_repo() -> Repo {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "// refquiem: allow(Nothing)\npub fn kept() {}\n",
    );
    repo.write("README.md", "# Fixture\n\nCall `fixture::gone`.\n");
    repo.write(
        "docs/old.md",
        &format!("{}`fixture::gone`\n", "\n".repeat(8)),
    );
    repo.commit("drop gone");
    repo
}

fn scan(repo: &Repo, range: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"))
        .args(["refquiem", "--json", "--no-fail"])
        .args(range)
        .current_dir(repo.path())
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Runs the review script in `mode` on the `report` a scan printed, returning the status it
/// recorded and the job summary.
fn review(
    gh: &FakeGh,
    mode: &str,
    dry_run: bool,
    payload: &Path,
    report: &str,
) -> (String, String) {
    review_with(gh, mode, dry_run, payload, report, &[])
}

/// [`review`] with extra environment variables.
fn review_with(
    gh: &FakeGh,
    mode: &str,
    dry_run: bool,
    payload: &Path,
    report: &str,
    env: &[(&str, &str)],
) -> (String, String) {
    let (success, status, summary) = run_review(gh, mode, dry_run, payload, report, env);
    assert!(
        success,
        "the review script failed with {status}\n{}",
        gh.log()
    );
    (status, summary)
}

/// Runs the review script, returning whether it exited successfully, the status it recorded
/// and the job summary.
fn run_review(
    gh: &FakeGh,
    mode: &str,
    dry_run: bool,
    payload: &Path,
    report: &str,
    env: &[(&str, &str)],
) -> (bool, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let report_file = dir.path().join("report.json");
    let status = dir.path().join("status");
    let summary = dir.path().join("summary.md");
    std::fs::write(&report_file, report).unwrap();
    std::fs::write(&summary, "").unwrap();
    let path = format!(
        "{}:{}",
        gh.dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let result = Command::new("bash")
        .arg(script("review.sh"))
        .env("PATH", path)
        .env("MODE", mode)
        .env("DRY_RUN", if dry_run { "true" } else { "false" })
        .env("PR_NUMBER", if mode == "post" { "" } else { "7" })
        .env("REPORT", &report_file)
        .env("PAYLOAD", payload)
        .env("STATUS", &status)
        .env("VERSION", "0.1.0")
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_SERVER_URL", "https://github.com")
        .env("GITHUB_RUN_ID", "42")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        result.status.success(),
        std::fs::read_to_string(&status).unwrap().trim().to_owned(),
        std::fs::read_to_string(&summary).unwrap(),
    )
}

/// The environment of the `workflow_run` job posting the payload of run 99.
const IN_WORKFLOW_RUN: &[(&str, &str)] = &[("EVENT_NAME", "workflow_run"), ("RUN_ID", "99")];

/// The payload a `prepare` run leaves for `report` on pull request 7.
fn prepared(report: &str) -> tempfile::TempDir {
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), report);
    assert_eq!(status, "prepared");
    payload
}

/// Runs the posting job on `payload`, returning whether it exited successfully and its status.
fn post(gh: &FakeGh, payload: &Path, env: &[(&str, &str)]) -> (bool, String) {
    let (success, status, _) = run_review(gh, "post", false, payload, "", env);
    (success, status)
}

/// Replaces `from` with `to` in every file of `payload`.
fn forge(payload: &Path, from: &str, to: &str) {
    for entry in std::fs::read_dir(payload).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace(from, to)).unwrap();
    }
}

/// Asserts the posting job refuses `payload` and touches no review.
fn assert_rejected(gh: &FakeGh, payload: &Path, env: &[(&str, &str)]) {
    let outcome = post(gh, payload, env);
    let log = gh.log();
    assert_eq!(outcome, (false, "rejected".to_owned()), "{log}");
    assert!(reviews_posted(&log).is_empty(), "{log}");
    assert_eq!(dismissed(&log), Vec::<&str>::new(), "{log}");
    assert!(!log.contains("graphql"), "{log}");
}

/// Every `gh` call in the log as its arguments and the stdin it read.
fn calls(log: &str) -> Vec<(&str, &str)> {
    log.split("CALL ")
        .filter(|call| !call.is_empty())
        .map(|call| {
            let call = call.strip_suffix("END\n").unwrap_or(call);
            let (args, stdin) = call.split_once('\n').unwrap_or((call, ""));
            (args, stdin.strip_suffix('\n').unwrap_or(stdin))
        })
        .collect()
}

/// The payloads of the reviews posted, in order.
fn reviews_posted(log: &str) -> Vec<serde_json::Value> {
    calls(log)
        .into_iter()
        .filter(|(args, _)| args.contains("-X POST") && args.contains("pulls/7/reviews"))
        .map(|(_, stdin)| serde_json::from_str(stdin).unwrap())
        .collect()
}

/// The payload of the last review posted.
fn last_review(log: &str) -> serde_json::Value {
    reviews_posted(log)
        .pop()
        .unwrap_or_else(|| panic!("no review posted\n{log}"))
}

/// The `(path, line)` of every comment of the last review on `side` of the diff, in order.
fn posted_on(log: &str, side: &str) -> Vec<(String, String)> {
    last_review(log)["comments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|comment| comment["side"] == side)
        .map(|comment| {
            (
                comment["path"].as_str().unwrap().to_owned(),
                comment["line"].to_string(),
            )
        })
        .collect()
}

/// The `(path, line)` of every comment posted on a line of the head, in order.
fn commented(log: &str) -> Vec<(String, String)> {
    posted_on(log, "RIGHT")
}

/// The `(path, line)` of every comment posted on a deleted line, in order.
fn on_deleted_lines(log: &str) -> Vec<(String, String)> {
    posted_on(log, "LEFT")
}

/// The body of the last review's comment on `path` at `line` on `side`.
fn comment_call(log: &str, path: &str, line: &str, side: &str) -> String {
    let line: u64 = line.parse().unwrap();
    last_review(log)["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| {
            comment["path"] == path && comment["line"] == line && comment["side"] == side
        })
        .and_then(|comment| comment["body"].as_str())
        .unwrap_or_else(|| panic!("no comment on {path}:{line} {side}\n{log}"))
        .to_owned()
}

/// The body of the last review posted.
fn review_call(log: &str) -> String {
    last_review(log)["body"].as_str().unwrap().to_owned()
}

/// The ids of the reviews dismissed, in order.
fn dismissed(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| l.starts_with("CALL ") && l.contains("/dismissals"))
        .filter_map(|l| l.split("pulls/7/reviews/").nth(1)?.split('/').next())
        .collect()
}

/// The ids each call to `mutation` named with `-f id=`, in order.
fn mutated<'a>(log: &'a str, mutation: &str) -> Vec<&'a str> {
    calls(log)
        .into_iter()
        .filter(|(args, _)| args.contains("graphql") && args.contains(mutation))
        .filter_map(|(args, _)| args.split(' ').find_map(|arg| arg.strip_prefix("id=")))
        .collect()
}

/// The ids of the review threads resolved, in order.
fn resolved(log: &str) -> Vec<&str> {
    mutated(log, "resolveReviewThread")
}

/// The ids of the review comments hidden, in order.
fn hidden(log: &str) -> Vec<&str> {
    mutated(log, "minimizeComment")
}

/// Asserts the run resolved no thread and hid no comment.
fn assert_no_thread_closed(log: &str) {
    assert_eq!(resolved(log), Vec::<&str>::new(), "{log}");
    assert_eq!(hidden(log), Vec::<&str>::new(), "{log}");
}

#[test]
fn a_review_comments_on_the_deleted_line_and_on_changed_files() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, summary) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), [pair("src/lib.rs", "1")]);
    assert_eq!(
        commented(&log),
        [pair("README.md", "3"), pair("src/lib.rs", "1")]
    );
    let removal = comment_call(&log, "src/lib.rs", "1", "LEFT");
    let blob = "https://github.com/owner/repo/blob/f00df00df00df00df00df00df00df00df00df00d";
    assert!(removal.contains(" (function), deleted in "), "{removal}");
    assert!(
        removal.contains("is still mentioned in these places."),
        "{removal}"
    );
    assert!(
        removal.contains(&format!("- [`README.md:3:7`]({blob}/README.md#L3)")),
        "{removal}"
    );
    assert!(
        removal.contains(&format!("- [`docs/old.md:9:2`]({blob}/docs/old.md#L9)")),
        "{removal}"
    );
    assert!(
        removal.contains(&format!("\n{blob}/README.md#L3\n{blob}/docs/old.md#L9\n")),
        "{removal}"
    );
    assert!(
        removal.contains("issues/new?template=wrong-review.yml"),
        "{removal}"
    );
    assert!(
        removal.contains("[❤️ Support refquiem](https://github.com/sponsors/LucaCappelletti94)"),
        "{removal}"
    );
    let inline = comment_call(&log, "README.md", "3", "RIGHT");
    assert!(inline.contains(", is still mentioned here."), "{inline}");
    assert!(
        comment_call(&log, "src/lib.rs", "1", "RIGHT")
            .contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{log}"
    );
    assert_eq!(reviews_posted(&log).len(), 1, "{log}");
    assert!(!log.contains("pulls/7/comments"), "{log}");
    let posted = last_review(&log);
    assert_eq!(
        posted["commit_id"],
        "f00df00df00df00df00df00df00df00df00df00d"
    );
    assert_eq!(posted["event"], "REQUEST_CHANGES");

    let review = review_call(&log);
    assert!(
        review.contains(
            "refquiem found 1 removed name still mentioned 2 times and 1 suppression marker that suppresses nothing."
        ),
        "{review}"
    );
    assert!(
        review.contains("- 1 with a suggested replacement"),
        "{review}"
    );
    assert!(!review.contains("not on a line"), "{review}");
    assert!(
        review.contains("issues/new?template=wrong-review.yml"),
        "{review}"
    );
    assert!(
        review.contains("[❤️ Support refquiem](https://github.com/sponsors/LucaCappelletti94)"),
        "{review}"
    );
    assert!(summary.contains("### refquiem review"), "{summary}");
}

#[test]
fn a_name_without_a_declaration_line_lists_its_mentions_in_the_body() {
    let report = stale_scan_of_the_full_history();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), Vec::<(String, String)>::new());
    let review = review_call(&log);
    let blob = "https://github.com/owner/repo/blob/f00df00df00df00df00df00df00df00df00df00d";
    assert!(
        review.contains("These are not on a line of this pull request diff"),
        "{review}"
    );
    assert!(
        review.contains(&format!(
            ", is still mentioned in [`docs/old.md:9:2`]({blob}/docs/old.md#L9)."
        )),
        "the one place left names itself in the sentence\n{review}"
    );
    assert!(!review.contains("these places"), "{review}");
    assert!(
        !review.contains("README.md:3:7"),
        "a mention commented inline stays out of the body\n{review}"
    );
}

/// The `--json --no-fail` scan of `fixture::gone`, deleted at `src/lib.rs:1` and mentioned
/// once, at `README.md:3:7`.
fn single_mention_scan() -> String {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write("README.md", "# Fixture\n\nCall `fixture::gone`.\n");
    repo.commit("drop gone");
    scan(&repo, &["--since", "HEAD~1"])
}

#[test]
fn a_name_mentioned_once_names_the_place_in_its_sentence() {
    let report = single_mention_scan();
    let gh = FakeGh::new(&["src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    let removal = comment_call(&log, "src/lib.rs", "1", "LEFT");
    let blob = "https://github.com/owner/repo/blob/f00df00df00df00df00df00df00df00df00df00d";
    assert!(
        removal.contains(&format!(
            ", is still mentioned in [`README.md:3:7`]({blob}/README.md#L3).\n\n{blob}/README.md#L3\n"
        )),
        "{removal}"
    );
    assert!(!removal.contains("these places"), "{removal}");
}

#[test]
fn an_inline_comment_carries_the_uppercase_hint_of_its_mention() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn array_agg() {}\n");
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nbuiltins = [\"sql@2023\"]\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write("README.md", "Run `array_agg`.\n");
    repo.commit("drop array_agg");
    let report = scan(&repo, &["--since", "HEAD~1"]);
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let inline = comment_call(&gh.log(), "README.md", "1", "RIGHT");
    assert!(
        inline.contains(", is still mentioned here. Write `ARRAY_AGG` to mark it as `sql@2023`."),
        "{inline}"
    );
}

#[test]
fn a_removal_comment_links_the_removed_name_to_the_base_and_its_replacement_to_head() {
    let report = single_mention_scan();
    let base: serde_json::Value = serde_json::from_str(&report).unwrap();
    let base = base["base"].as_str().unwrap().to_owned();
    let gh = FakeGh::new(&["src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    let removal = comment_call(&log, "src/lib.rs", "1", "LEFT");
    let blob = "https://github.com/owner/repo/blob";
    let head = "f00df00df00df00df00df00df00df00df00df00d";
    assert!(
        removal.contains(&format!(
            "[`fixture::gone`]({blob}/{base}/src/lib.rs#L1) (function), deleted in "
        )),
        "{removal}"
    );
    assert!(
        removal.contains(&format!(
            ", replaced by [`fixture::kept`]({blob}/{head}/src/lib.rs#L1), is still mentioned in "
        )),
        "{removal}"
    );
}

#[test]
fn a_single_finding_its_comment_holds_leaves_the_review_body_bare() {
    let gh = FakeGh::new(&["src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &single_mention_scan());
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(
        on_deleted_lines(&log),
        [("src/lib.rs".to_owned(), "1".to_owned())]
    );
    let review = review_call(&log);
    for repeated in ["refquiem found", "Report a wrong review", "[Run]"] {
        assert!(!review.contains(repeated), "{repeated}\n{review}");
    }
    assert!(
        review.contains("<!-- refquiem -->"),
        "later runs find the review by its marker\n{review}"
    );
}

/// A hunk far below every line the fixtures place a finding on.
const ELSEWHERE: &str = "@@ -50 +50 @@\n-old\n+new";

#[test]
fn a_single_finding_no_comment_holds_stays_in_the_review_body() {
    let gh = FakeGh::with_patches(&[("src/lib.rs", ELSEWHERE)], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &single_mention_scan());
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(last_review(&log)["comments"], serde_json::json!([]));
    let review = review_call(&log);
    assert!(
        review.contains("refquiem found 1 removed name still mentioned 1 time."),
        "{review}"
    );
    assert!(review.contains("README.md:3:7"), "{review}");
}

#[test]
fn a_deleted_line_outside_the_hunks_moves_its_mentions_to_the_review_body() {
    let report = stale_scan();
    let gh = FakeGh::with_patches(
        &[
            (
                "README.md",
                "@@ -1,3 +1,3 @@\n # Fixture\n \n Call `fixture::gone`.",
            ),
            ("src/lib.rs", ELSEWHERE),
        ],
        None,
    );
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), Vec::<(String, String)>::new());
    assert_eq!(commented(&log), [pair("README.md", "3")]);
    let review = review_call(&log);
    assert!(review.contains("docs/old.md:9:2"), "{review}");
    assert!(!review.contains("README.md:3:7"), "{review}");
    assert!(
        review.contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{review}"
    );
}

#[test]
fn a_mention_outside_the_hunks_stays_with_its_deleted_line_comment() {
    let report = stale_scan();
    let gh = FakeGh::with_patches(
        &[
            ("README.md", ELSEWHERE),
            (
                "src/lib.rs",
                "@@ -1 +1,2 @@\n-pub fn gone() {}\n+// refquiem: allow(Nothing)\n+pub fn kept() {}",
            ),
        ],
        None,
    );
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), [pair("src/lib.rs", "1")]);
    assert_eq!(commented(&log), [pair("src/lib.rs", "1")]);
    let review = review_call(&log);
    assert!(!review.contains("README.md:3:7"), "{review}");
    assert!(!review.contains("not on a line"), "{review}");
}

#[test]
fn a_mention_on_a_context_line_of_a_later_hunk_is_commented() {
    let report = stale_scan();
    let gh = FakeGh::with_patches(
        &[(
            "README.md",
            "@@ -1 +1 @@\n-# Old\n+# Fixture\n@@ -3 +3 @@\n Call `fixture::gone`.\n\\ No newline at end of file",
        )],
        None,
    );
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    assert_eq!(commented(&gh.log()), [pair("README.md", "3")]);
}

#[test]
fn a_deleted_line_shown_only_as_context_takes_no_comment() {
    let report = single_mention_scan();
    let gh = FakeGh::with_patches(
        &[("src/lib.rs", "@@ -1,2 +1,2 @@\n pub fn gone() {}\n-a\n+b")],
        None,
    );
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), Vec::<(String, String)>::new());
    assert!(review_call(&log).contains("README.md:3:7"), "{log}");
}

#[test]
fn a_file_without_a_patch_keeps_its_findings_in_the_body() {
    let report = stale_scan();
    let gh = FakeGh::new(&[], None);
    std::fs::write(
        gh.dir.path().join("files.ndjson"),
        "{\"filename\":\"src/lib.rs\",\"patch\":null}\n{\"filename\":\"README.md\"}\n",
    )
    .unwrap();
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(last_review(&log)["comments"], serde_json::json!([]));
    for place in ["README.md:3:7", "docs/old.md:9:2", "src/lib.rs:1:20"] {
        assert!(review_call(&log).contains(place), "{place}\n{log}");
    }
}

#[test]
fn new_and_deleted_files_use_their_own_side_of_the_diff() {
    let report = stale_scan();
    let gh = FakeGh::with_patches(
        &[
            (
                "README.md",
                "@@ -0,0 +1,3 @@\n+# Fixture\n+\n+Call `fixture::gone`.",
            ),
            (
                "src/lib.rs",
                "@@ -1 +0,0 @@\n-pub fn gone() {}\n\\ No newline at end of file",
            ),
        ],
        None,
    );
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(on_deleted_lines(&log), [pair("src/lib.rs", "1")]);
    assert_eq!(commented(&log), [pair("README.md", "3")]);
    assert!(review_call(&log).contains("src/lib.rs:1:20"), "{log}");
}

#[test]
fn a_review_github_rejects_is_posted_again_with_every_finding_in_its_body() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("review-comments", 422)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    let posted = reviews_posted(&log);
    assert_eq!(posted.len(), 2, "{log}");
    assert_ne!(posted[0]["comments"], serde_json::json!([]));
    assert_eq!(posted[1]["comments"], serde_json::json!([]));
    let review = review_call(&log);
    for place in ["README.md:3:7", "docs/old.md:9:2", "src/lib.rs:1:20"] {
        assert!(review.contains(place), "{place}\n{review}");
    }
    assert_eq!(dismissed(&log), ["101"]);
    // The body-only review anchors nothing, so the earlier threads still point at live findings.
    assert_no_thread_closed(&log);
}

#[test]
fn a_review_github_fails_to_post_fails_the_run_and_dismisses_nothing() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("review", 500)));
    let payload = tempfile::tempdir().unwrap();
    let (success, status, _) = run_review(&gh, "review", false, payload.path(), &report, &[]);
    assert!(!success);
    assert_eq!(status, "failed");
    let log = gh.log();
    assert_eq!(reviews_posted(&log).len(), 1, "{log}");
    assert_eq!(dismissed(&log), Vec::<&str>::new());
    assert_no_thread_closed(&log);
}

#[test]
fn a_read_only_token_skips_the_review_and_dismisses_nothing() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("review", 403)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "skipped-readonly");
    let log = gh.log();
    assert_eq!(reviews_posted(&log).len(), 1, "{log}");
    assert_eq!(dismissed(&log), Vec::<&str>::new());
    assert_no_thread_closed(&log);
}

#[test]
fn a_dry_run_renders_every_comment_into_the_summary_and_posts_nothing() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, summary) = review(&gh, "review", true, payload.path(), &report);
    assert_eq!(status, "dry-run");
    assert!(!gh.log().contains("comments"), "{}", gh.log());
    assert!(!gh.log().contains("reviews"), "{}", gh.log());
    assert_eq!(
        summary.matches(", is still mentioned here.").count(),
        2,
        "{summary}"
    );
    assert!(
        summary.contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{summary}"
    );
    assert!(
        summary.contains("#### `src/lib.rs:1` deleted line"),
        "{summary}"
    );
    assert!(
        summary.contains("#### Review body\n\nrefquiem found 1 removed name"),
        "{summary}"
    );
}

#[test]
fn a_dry_run_without_a_pull_request_reports_wrong_reviews_against_the_run() {
    let report = stale_scan();
    let gh = FakeGh::new(&[], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, summary) = review_with(
        &gh,
        "review",
        true,
        payload.path(),
        &report,
        &[("PR_NUMBER", "")],
    );
    assert_eq!(status, "dry-run");
    assert!(
        summary.contains("title=Wrong%20review%20on%20owner%2Frepo&"),
        "{summary}"
    );
    assert!(
        summary.contains("review=https%3A%2F%2Fgithub.com%2Fowner%2Frepo%2Factions%2Fruns%2F42&"),
        "{summary}"
    );
}

#[test]
fn prepare_then_post_posts_the_review_a_read_only_job_computed() {
    let report = stale_scan();
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), &report);
    assert_eq!(status, "prepared");
    assert!(!preparing.log().contains("comments"), "{}", preparing.log());
    assert!(!preparing.log().contains("reviews"), "{}", preparing.log());

    let posting = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let (status, _) = review_with(&posting, "post", false, payload.path(), "", IN_WORKFLOW_RUN);
    assert_eq!(status, "posted");
    let log = posting.log();
    assert_eq!(on_deleted_lines(&log), [pair("src/lib.rs", "1")]);
    assert_eq!(
        commented(&log),
        [pair("README.md", "3"), pair("src/lib.rs", "1")]
    );
    assert!(
        log.contains("pulls?state=open&head=fork%3Afeature"),
        "the run's own head names the pull request\n{log}"
    );
    let review = review_call(&log);
    assert!(
        review.contains("version=0.1.0&"),
        "the post job keeps the prepared version\n{review}"
    );
    assert!(!review.contains("not on a line"), "{review}");
}

#[test]
fn a_prepared_review_in_dry_run_posts_nothing() {
    let report = stale_scan();
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), &report);
    assert_eq!(status, "prepared");

    let posting = FakeGh::new(&[], None);
    let (status, summary) =
        review_with(&posting, "post", true, payload.path(), "", IN_WORKFLOW_RUN);
    let log = posting.log();
    assert!(!log.contains("comments"), "{log}");
    assert!(!log.contains("reviews"), "{log}");
    assert_eq!(status, "dry-run");
    assert!(summary.contains("README.md:3:7"), "{summary}");
}

/// A prepared payload comes from a run anyone can trigger, so it posts only on the pull
/// request whose head is the commit that run checked out.
#[test]
fn post_skips_a_payload_whose_pull_request_is_not_at_the_run_head() {
    let report = stale_scan();
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md", "src/lib.rs"], None);
    review(&preparing, "prepare", false, payload.path(), &report);

    let posting = FakeGh::new(&[], None);
    posting.run(
        &serde_json::json!({"event": "pull_request"}),
        "0123456789012345678901234567890123456789",
    );
    let (status, _) = review_with(&posting, "post", false, payload.path(), "", IN_WORKFLOW_RUN);
    assert_eq!(status, "skipped-stale");
    assert!(!posting.log().contains("comments"), "{}", posting.log());
    assert!(!posting.log().contains("reviews"), "{}", posting.log());
}

#[test]
fn post_without_a_prepared_review_fails() {
    let gh = FakeGh::new(&[], None);
    let payload = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let status = dir.path().join("status");
    let result = Command::new("bash")
        .arg(script("review.sh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                gh.dir.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("MODE", "post")
        .env("DRY_RUN", "false")
        .env("PAYLOAD", payload.path())
        .env("STATUS", &status)
        .env("GITHUB_REPOSITORY", "owner/repo")
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(&status).unwrap().trim(), "failed");
}

/// Two pull requests can share a head commit, so the payload posts only on one the run built.
#[test]
fn post_rejects_a_payload_naming_a_pull_request_the_run_did_not_build() {
    let payload = prepared(&stale_scan());
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    gh.open(&[8]);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
}

#[test]
fn post_rejects_a_clean_payload_for_a_pull_request_the_run_did_not_build() {
    let payload = prepared(&clean_scan());
    let gh = FakeGh::new(&["README.md"], None);
    gh.open(&[8]);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
}

#[test]
fn post_rejects_a_pr_number_input_naming_another_pull_request() {
    let payload = prepared(&stale_scan());
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let env = [IN_WORKFLOW_RUN, &[("PR_NUMBER", "8")]].concat();
    assert_rejected(&gh, payload.path(), &env);
}

#[test]
fn post_rejects_a_run_that_did_not_come_from_a_pull_request() {
    let payload = prepared(&stale_scan());
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    gh.run(&serde_json::json!({"event": "push"}), HEAD);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
}

#[test]
fn post_fails_when_the_run_cannot_be_read() {
    let payload = prepared(&stale_scan());
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("runs", 500)));
    let (success, status) = post(&gh, payload.path(), IN_WORKFLOW_RUN);
    assert!(!success);
    assert_eq!(status, "failed");
    assert!(reviews_posted(&gh.log()).is_empty(), "{}", gh.log());
}

/// Outside `workflow_run` no run vouches for the payload, so the job names the pull request.
#[test]
fn post_outside_workflow_run_needs_the_pull_request_named() {
    let payload = prepared(&stale_scan());
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    assert_rejected(&gh, payload.path(), &[("EVENT_NAME", "push")]);

    let named = [("EVENT_NAME", "push"), ("PR_NUMBER", "7")];
    assert_eq!(
        post(&gh, payload.path(), &named),
        (true, "posted".to_owned())
    );
}

/// The review is rendered from the report's fields, so prepared text that differs is refused.
#[test]
fn post_rejects_text_the_report_fields_do_not_produce() {
    let payload = prepared(&stale_scan());
    forge(
        payload.path(),
        "(function), deleted in",
        "(function) [a link](https://example.com/phish), deleted in",
    );
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
    assert!(!gh.log().contains("phish"), "{}", gh.log());
}

/// A backtick would end the code span a name renders in and let markdown through.
#[test]
fn post_rejects_markdown_in_a_reported_name() {
    let payload = prepared(&stale_scan());
    forge(
        payload.path(),
        "fixture::gone",
        "fixture::gone` [a link](https://example.com/phish) `",
    );
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
}

#[test]
fn post_rejects_an_oversized_payload() {
    let payload = prepared(&stale_scan());
    for entry in std::fs::read_dir(payload.path()).unwrap() {
        let path = entry.unwrap().path();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&" ".repeat(16 << 20));
        std::fs::write(&path, text).unwrap();
    }
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    assert_rejected(&gh, payload.path(), IN_WORKFLOW_RUN);
}

/// The posting job renders what a job reviewing its own pull request posts.
#[test]
fn review_and_post_post_the_same_review() {
    let report = stale_scan();
    let reviewing = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    review(&reviewing, "review", false, payload.path(), &report);

    let posting = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = prepared(&report);
    assert_eq!(
        post(&posting, payload.path(), IN_WORKFLOW_RUN),
        (true, "posted".to_owned())
    );
    assert_eq!(last_review(&posting.log()), last_review(&reviewing.log()));
}

/// Every headline the scanner writes is one the review rebuilds from its fields.
#[test]
fn every_headline_shape_renders() {
    let reworked = Repo::new();
    reworked.write(
        "src/lib.rs",
        "pub mod a {\n    pub fn shifted() {}\n}\npub fn colour() {}\npub fn gone(x: u32) {}\n",
    );
    reworked.commit("start");
    reworked.write(
        "src/lib.rs",
        "pub mod b {\n    pub fn shifted() {}\n}\npub fn color() {}\npub fn first(x: u32) {}\npub fn second(x: u32) {}\n",
    );
    reworked.write(
        "README.md",
        "`fixture::a::shifted` `fixture::colour` `fixture::gone`\n",
    );
    reworked.commit("rework");

    let members = Repo::new();
    members.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    for member in ["alpha", "beta"] {
        members.write(
            &format!("crates/{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nx = []\n"),
        );
        members.write(&format!("crates/{member}/src/lib.rs"), "");
    }
    members.commit("two members");
    members.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    members.write("crates/alpha/README.md", "`--features x`\n");

    for (report, headlines) in [
        (
            scan(&reworked, &["--since", "HEAD~1"]),
            &[
                "moved to [`fixture::b::shifted`]",
                "renamed to [`fixture::color`]",
                ", perhaps `fixture::first` or `fixture::second`",
            ][..],
        ),
        (
            scan(&members, &[]),
            &["`x` (feature of `alpha`), removed in the working tree"][..],
        ),
    ] {
        let gh = FakeGh::new(&[], None);
        let payload = tempfile::tempdir().unwrap();
        let (status, summary) = review(&gh, "review", true, payload.path(), &report);
        assert_eq!(status, "dry-run", "{summary}");
        for headline in headlines {
            assert!(summary.contains(headline), "{headline}\n{summary}");
        }
    }
}

#[test]
fn a_review_whose_changed_files_cannot_be_read_fails_without_posting() {
    let gh = FakeGh::new(&["README.md"], Some(("files", 500)));
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.json");
    let status = dir.path().join("status");
    std::fs::write(&report, stale_scan()).unwrap();
    let result = Command::new("bash")
        .arg(script("review.sh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                gh.dir.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("MODE", "review")
        .env("DRY_RUN", "false")
        .env("PR_NUMBER", "7")
        .env("REPORT", &report)
        .env("PAYLOAD", dir.path().join("payload"))
        .env("STATUS", &status)
        .env("GITHUB_REPOSITORY", "owner/repo")
        .env("GITHUB_SERVER_URL", "https://github.com")
        .env("GITHUB_STEP_SUMMARY", dir.path().join("summary.md"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read_to_string(&status).unwrap().trim(), "failed");
    assert!(!gh.log().contains("comments"), "{}", gh.log());
    assert!(!gh.log().contains("reviews"), "{}", gh.log());
}

/// The `--json --no-fail` scan of a repository with nothing stale.
fn clean_scan() -> String {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");
    repo.commit("nothing");
    scan(&repo, &["--since", "HEAD~1"])
}

#[test]
fn a_clean_run_dismisses_only_its_own_change_requests() {
    let gh = FakeGh::new(&["README.md"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &clean_scan());
    assert_eq!(status, "skipped-clean");
    let log = gh.log();
    assert!(!log.contains("-X POST"), "{log}");
    assert_eq!(dismissed(&log), ["101"]);
    assert!(log.contains("event=DISMISS"), "{log}");
}

#[test]
fn a_clean_run_closes_every_open_thread_of_its_earlier_reviews() {
    let gh = FakeGh::new(&["README.md"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &clean_scan());
    assert_eq!(status, "skipped-clean");
    let log = gh.log();
    // T1 holds a reply and closes too, the maintainer's T3 and the resolved T4 stay as they are.
    assert_eq!(resolved(&log), ["T1", "T2"], "{log}");
    assert_eq!(hidden(&log), Vec::<&str>::new(), "{log}");
}

#[test]
fn a_posted_review_closes_the_earlier_threads_and_keeps_its_own() {
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &stale_scan());
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(
        resolved(&log),
        ["T1", "T2"],
        "the new review's T5 stays open\n{log}"
    );
    assert!(
        log.find("-X POST").unwrap() < log.find("resolveReviewThread").unwrap(),
        "{log}"
    );
}

#[test]
fn a_refused_resolve_hides_every_comment_of_the_earlier_threads_as_outdated() {
    let gh = FakeGh::new(&["README.md"], Some(("resolve", 0)));
    let payload = tempfile::tempdir().unwrap();
    let (success, status, _) = run_review(&gh, "review", false, payload.path(), &clean_scan(), &[]);
    assert!(success);
    assert_eq!(status, "skipped-clean");
    let log = gh.log();
    assert_eq!(resolved(&log), ["T1"], "one refusal is enough\n{log}");
    assert_eq!(hidden(&log), ["C11", "C12", "C21"], "{log}");
    for (args, _) in calls(&log)
        .into_iter()
        .filter(|(args, _)| args.contains("minimizeComment"))
    {
        assert!(args.contains("classifier: OUTDATED"), "{args}");
    }
}

#[test]
fn a_refused_hide_leaves_the_run_status_as_it_was() {
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("close", 0)));
    let payload = tempfile::tempdir().unwrap();
    let (success, status, _) = run_review(&gh, "review", false, payload.path(), &stale_scan(), &[]);
    assert!(success);
    assert_eq!(status, "posted");
    assert_eq!(hidden(&gh.log()), ["C11", "C12", "C21"], "{}", gh.log());
}

#[test]
fn review_threads_that_cannot_be_read_leave_the_run_status_as_it_was() {
    let gh = FakeGh::new(&["README.md"], Some(("threads", 502)));
    let payload = tempfile::tempdir().unwrap();
    let (success, status, _) = run_review(&gh, "review", false, payload.path(), &clean_scan(), &[]);
    assert!(success);
    assert_eq!(status, "skipped-clean");
    let log = gh.log();
    assert_eq!(dismissed(&log), ["101"]);
    assert_no_thread_closed(&log);
}

#[test]
fn a_run_with_findings_dismisses_its_earlier_request_once_its_own_lands() {
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &stale_scan());
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(dismissed(&log), ["101"], "the new review 104 stays");
    assert!(
        log.find("-X POST").unwrap() < log.find("/dismissals").unwrap(),
        "{log}"
    );
    assert!(review_call(&log).contains("<!-- refquiem -->"), "{log}");
}

#[test]
fn a_dismissal_the_token_may_not_make_is_skipped() {
    let gh = FakeGh::new(&["README.md"], Some(("dismissals", 403)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &clean_scan());
    assert_eq!(status, "skipped-clean");
}

#[test]
fn a_clean_prepared_review_in_a_dry_run_dismisses_nothing() {
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), &clean_scan());
    assert_eq!(status, "prepared");

    let dry = FakeGh::new(&[], None);
    let (status, _) = review_with(&dry, "post", true, payload.path(), "", IN_WORKFLOW_RUN);
    assert_eq!(status, "skipped-clean");
    assert!(!dry.log().contains("reviews"), "{}", dry.log());
}

#[test]
fn a_clean_dry_run_touches_no_review() {
    let gh = FakeGh::new(&["README.md"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", true, payload.path(), &clean_scan());
    assert_eq!(status, "skipped-clean");
    assert!(!gh.log().contains("reviews"), "{}", gh.log());
    assert!(!gh.log().contains("graphql"), "{}", gh.log());
}

#[test]
fn a_clean_prepare_lets_the_post_job_dismiss_at_the_run_head() {
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), &clean_scan());
    assert_eq!(status, "prepared");
    assert!(!preparing.log().contains("reviews"), "{}", preparing.log());

    let moved_on = FakeGh::new(&[], None);
    moved_on.run(
        &serde_json::json!({"event": "pull_request"}),
        "0123456789012345678901234567890123456789",
    );
    let (status, _) = review_with(
        &moved_on,
        "post",
        false,
        payload.path(),
        "",
        IN_WORKFLOW_RUN,
    );
    assert_eq!(status, "skipped-stale");
    assert_eq!(dismissed(&moved_on.log()), Vec::<&str>::new());
    assert_no_thread_closed(&moved_on.log());

    let posting = FakeGh::new(&[], None);
    let (status, _) = review_with(&posting, "post", false, payload.path(), "", IN_WORKFLOW_RUN);
    assert_eq!(status, "skipped-clean");
    let log = posting.log();
    assert_eq!(dismissed(&log), ["101"]);
    assert_eq!(resolved(&log), ["T1", "T2"], "{log}");
    assert!(!log.contains("-X POST"), "{log}");
}

#[test]
fn a_line_with_rewrites_becomes_one_comment_with_a_suggestion_block() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write(
        "README.md",
        "Call `fixture::gone` or `fixture::gone`.\n\nThe ```` fence `fixture::gone`.\n",
    );
    repo.commit("replace gone");
    let report = scan(&repo, &["--since", "HEAD~1"]);
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(
        commented(&log),
        [pair("README.md", "1"), pair("README.md", "3")]
    );
    let first = comment_call(&log, "README.md", "1", "RIGHT");
    assert_eq!(
        first.matches("is still mentioned here.").count(),
        1,
        "{first}"
    );
    assert!(
        first.contains("\n\n```suggestion\nCall `fixture::kept` or `fixture::kept`.\n```\n\n"),
        "{first}"
    );
    let third = comment_call(&log, "README.md", "3", "RIGHT");
    assert!(
        third.contains("\n\n`````suggestion\nThe ```` fence `fixture::kept`.\n`````\n\n"),
        "{third}"
    );
    let removal = comment_call(&log, "src/lib.rs", "1", "LEFT");
    let blob = "https://github.com/owner/repo/blob/f00df00df00df00df00df00df00df00df00df00d";
    assert_eq!(
        removal.matches(&format!("\n{blob}/README.md#L1\n")).count(),
        1,
        "two mentions on one line embed it once\n{removal}"
    );
}
