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

impl FakeGh {
    /// `changed` are the pull request's files, `fail` makes posting a comment on that path
    /// fail with that status.
    fn new(changed: &[&str], fail: Option<(&str, u16)>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (fail_path, fail_status) = fail.map_or((String::new(), 0), |(p, s)| (p.to_owned(), s));
        let script = format!(
            r#"#!/usr/bin/env bash
log="{log}"
body=''
for arg in "$@"; do
  [ "$arg" = 'body=@-' ] && body="$(cat)"
done
printf 'CALL %s\n%s\nEND\n' "$*" "$body" >> "$log"
case "$*" in
  *pulls/7/comments*)
    if [ -n '{fail_path}' ] && [[ "$*" == *'path={fail_path} '* ]]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    echo '{{}}' ;;
  *pulls/7/reviews*) echo '{{"state": "CHANGES_REQUESTED"}}' ;;
  *pulls/7/files*) printf '%s\n' {changed} ;;
  *pulls/7*) echo 'f00df00df00df00df00df00df00df00df00df00d' ;;
  *) echo "unexpected gh $*" >&2; exit 1 ;;
esac
"#,
            log = dir.path().join("log").display(),
            changed = changed.join(" "),
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
}

/// The `--json --no-fail` scan of a repository whose `fixture::gone` is mentioned at
/// `README.md:3:7` and `docs/old.md:9:2`, with an unused marker at `src/lib.rs:1:20`.
fn stale_scan() -> String {
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
    scan(&repo)
}

fn scan(repo: &Repo) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"))
        .args(["refquiem", "--json", "--no-fail", "--since", "HEAD~1"])
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
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    (
        std::fs::read_to_string(&status).unwrap().trim().to_owned(),
        std::fs::read_to_string(&summary).unwrap(),
    )
}

/// The `(path, line)` of every comment posted, in order.
fn commented(log: &str) -> Vec<(String, String)> {
    log.lines()
        .filter(|l| l.starts_with("CALL ") && l.contains("pulls/7/comments"))
        .map(|l| {
            let field = |key: &str| {
                l.split(' ')
                    .find_map(|part| part.strip_prefix(key))
                    .unwrap()
                    .to_owned()
            };
            (field("path="), field("line="))
        })
        .collect()
}

/// The review call in the log, with its fields.
fn review_call(log: &str) -> &str {
    log.split("CALL ")
        .find(|call| call.contains("pulls/7/reviews"))
        .unwrap_or_else(|| panic!("no review posted\n{log}"))
}

#[test]
fn a_review_comments_on_changed_files_and_lists_the_rest() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, summary) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");

    let log = gh.log();
    assert_eq!(
        commented(&log),
        [pair("README.md", "3"), pair("src/lib.rs", "1")]
    );
    assert!(
        log.contains("`fixture::gone` (function), deleted in "),
        "{log}"
    );
    assert!(log.contains(", is still mentioned here."), "{log}");
    assert!(
        log.contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{log}"
    );
    assert!(
        log.contains("https://github.com/LucaCappelletti94/cargo-refquiem/issues/new?template=wrong-review.yml"),
        "{log}"
    );
    assert!(
        log.contains("[❤️ Support refquiem](https://github.com/sponsors/LucaCappelletti94)"),
        "{log}"
    );
    assert!(
        log.contains("commit_id=f00df00df00df00df00df00df00df00df00df00d"),
        "{log}"
    );
    let review = review_call(&log);
    assert!(review.contains("event=REQUEST_CHANGES"), "{review}");
    assert!(review.contains("docs/old.md:9:2"), "{review}");
    assert!(!review.contains("README.md:3:7"), "{review}");
    assert!(summary.contains("docs/old.md:9:2"), "{summary}");
}

#[test]
fn a_comment_github_cannot_anchor_moves_to_the_review_body() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("README.md", 422)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    assert!(
        review_call(&gh.log()).contains("README.md:3:7"),
        "{}",
        gh.log()
    );
}

#[test]
fn a_comment_failing_for_another_reason_is_listed_as_not_posted() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("src/lib.rs", 500)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    let review = review_call(&log);
    assert!(
        review.contains("These comments could not be posted."),
        "{review}"
    );
    assert!(review.contains("src/lib.rs:1:20"), "{review}");
}

#[test]
fn a_read_only_token_skips_the_review() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("README.md", 403)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "skipped-readonly");
    assert_eq!(commented(&gh.log()), [pair("README.md", "3")]);
    assert!(!gh.log().contains("pulls/7/reviews"), "{}", gh.log());
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

    let posting = FakeGh::new(&[], None);
    let (status, _) = review_with(
        &posting,
        "post",
        false,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "f00df00df00df00df00df00df00df00df00df00d")],
    );
    assert_eq!(status, "posted");
    let log = posting.log();
    assert_eq!(
        commented(&log),
        [pair("README.md", "3"), pair("src/lib.rs", "1")]
    );
    assert!(review_call(&log).contains("docs/old.md:9:2"), "{log}");
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
    let (status, _) = review_with(
        &posting,
        "post",
        false,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "0123456789012345678901234567890123456789")],
    );
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

#[test]
fn a_clean_scan_posts_nothing() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");
    repo.commit("nothing");
    let report = scan(&repo);
    let gh = FakeGh::new(&["README.md"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "skipped-clean");
    assert!(!gh.log().contains("comments"), "{}", gh.log());
    assert!(!gh.log().contains("reviews"), "{}", gh.log());
}
