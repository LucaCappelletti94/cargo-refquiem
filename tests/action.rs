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

/// The reviews the pull request already has: one of ours requesting changes, one of ours
/// already dismissed, and a maintainer's request without the marker.
const EARLIER_REVIEWS: &str = r#"{"id":101,"state":"CHANGES_REQUESTED","body":"stale\n\n<!-- refquiem -->"}
{"id":102,"state":"DISMISSED","body":"older\n\n<!-- refquiem -->"}
{"id":103,"state":"CHANGES_REQUESTED","body":"a maintainer's request"}"#;

impl FakeGh {
    /// `changed` are the pull request's files, `fail` makes posting a comment on that path,
    /// listing the files for `files`, or a dismissal for `dismissals`, fail with that status.
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
  *pulls/7/reviews/*/dismissals*)
    if [ '{fail_path}' = dismissals ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    echo '{{"state": "DISMISSED"}}' ;;
  *'-X POST'*pulls/7/reviews*) echo '{{"state": "CHANGES_REQUESTED"}}' ;;
  *pulls/7/reviews*) cat <<'REVIEWS'
{reviews}
REVIEWS
    ;;
  *pulls/7/files*)
    if [ '{fail_path}' = files ]; then
      echo 'gh: HTTP {fail_status}' >&2
      exit 1
    fi
    printf '%s\n' {changed} ;;
  *pulls/7*) echo 'f00df00df00df00df00df00df00df00df00df00d' ;;
  *) echo "unexpected gh $*" >&2; exit 1 ;;
esac
"#,
            log = dir.path().join("log").display(),
            changed = changed.join(" "),
            reviews = EARLIER_REVIEWS,
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

/// The `(path, line)` of every comment posted on `side` of the diff, in order.
fn posted_on(log: &str, side: &str) -> Vec<(String, String)> {
    log.lines()
        .filter(|l| l.starts_with("CALL ") && l.contains("pulls/7/comments"))
        .filter(|l| l.contains(&format!("side={side} ")))
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

/// The `(path, line)` of every comment posted on a line of the head, in order.
fn commented(log: &str) -> Vec<(String, String)> {
    posted_on(log, "RIGHT")
}

/// The `(path, line)` of every comment posted on a deleted line, in order.
fn on_deleted_lines(log: &str) -> Vec<(String, String)> {
    posted_on(log, "LEFT")
}

/// The posted comment call on `path` at `line` on `side`, with its body.
fn comment_call<'l>(log: &'l str, path: &str, line: &str, side: &str) -> &'l str {
    log.split("CALL ")
        .find(|call| {
            call.contains("pulls/7/comments")
                && call.contains(&format!("path={path} "))
                && call.contains(&format!("line={line} "))
                && call.contains(&format!("side={side} "))
        })
        .unwrap_or_else(|| panic!("no comment on {path}:{line} {side}\n{log}"))
}

/// The posted review call in the log, with its fields.
fn review_call(log: &str) -> &str {
    log.split("CALL ")
        .find(|call| call.contains("-X POST") && call.contains("pulls/7/reviews"))
        .unwrap_or_else(|| panic!("no review posted\n{log}"))
}

/// The ids of the reviews dismissed, in order.
fn dismissed(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|l| l.starts_with("CALL ") && l.contains("/dismissals"))
        .filter_map(|l| l.split("pulls/7/reviews/").nth(1)?.split('/').next())
        .collect()
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
    assert!(
        removal.contains("`fixture::gone` (function), deleted in "),
        "{removal}"
    );
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
        log.contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{log}"
    );
    assert!(
        log.contains("commit_id=f00df00df00df00df00df00df00df00df00df00d"),
        "{log}"
    );

    let review = review_call(&log);
    assert!(review.contains("event=REQUEST_CHANGES"), "{review}");
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

#[test]
fn a_single_finding_no_comment_holds_stays_in_the_review_body() {
    let gh = FakeGh::new(&["src/lib.rs"], Some(("src/lib.rs", 422)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &single_mention_scan());
    assert_eq!(status, "posted");

    let log = gh.log();
    let review = review_call(&log);
    assert!(
        review.contains("refquiem found 1 removed name still mentioned 1 time."),
        "{review}"
    );
    assert!(review.contains("README.md:3:7"), "{review}");
}

#[test]
fn a_deleted_line_github_rejects_moves_its_mentions_to_the_review_body() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("src/lib.rs", 422)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    let review = review_call(&log);
    assert!(review.contains("docs/old.md:9:2"), "{review}");
    assert!(
        review.contains("This `refquiem: allow(Nothing)` marker suppresses nothing."),
        "{review}"
    );
    assert!(!review.contains("could not be posted"), "{review}");
}

#[test]
fn a_mention_github_rejects_stays_with_its_deleted_line_comment() {
    let report = stale_scan();
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], Some(("README.md", 422)));
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &report);
    assert_eq!(status, "posted");
    let log = gh.log();
    let review = review_call(&log);
    assert!(!review.contains("README.md:3:7"), "{review}");
    assert!(!review.contains("not on a line"), "{review}");
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
    assert_eq!(on_deleted_lines(&log), [pair("src/lib.rs", "1")]);
    assert_eq!(
        commented(&log),
        [pair("README.md", "3"), pair("src/lib.rs", "1")]
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
    let (status, summary) = review_with(
        &posting,
        "post",
        true,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "f00df00df00df00df00df00df00df00df00df00d")],
    );
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
    assert!(!log.contains("comments"), "{log}");
    assert!(!log.contains("-X POST"), "{log}");
    assert_eq!(dismissed(&log), ["101"]);
    assert!(log.contains("event=DISMISS"), "{log}");
}

#[test]
fn a_run_with_findings_dismisses_its_earlier_request_before_posting() {
    let gh = FakeGh::new(&["README.md", "src/lib.rs"], None);
    let payload = tempfile::tempdir().unwrap();
    let (status, _) = review(&gh, "review", false, payload.path(), &stale_scan());
    assert_eq!(status, "posted");
    let log = gh.log();
    assert_eq!(dismissed(&log), ["101"]);
    assert!(
        log.find("/dismissals").unwrap() < log.find("-X POST").unwrap(),
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
    let (status, _) = review_with(
        &dry,
        "post",
        true,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "f00df00df00df00df00df00df00df00df00df00d")],
    );
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
}

#[test]
fn a_clean_prepare_lets_the_post_job_dismiss_at_the_run_head() {
    let payload = tempfile::tempdir().unwrap();
    let preparing = FakeGh::new(&["README.md"], None);
    let (status, _) = review(&preparing, "prepare", false, payload.path(), &clean_scan());
    assert_eq!(status, "prepared");
    assert!(!preparing.log().contains("reviews"), "{}", preparing.log());

    let moved_on = FakeGh::new(&[], None);
    let (status, _) = review_with(
        &moved_on,
        "post",
        false,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "0123456789012345678901234567890123456789")],
    );
    assert_eq!(status, "skipped-stale");
    assert_eq!(dismissed(&moved_on.log()), Vec::<&str>::new());

    let posting = FakeGh::new(&[], None);
    let (status, _) = review_with(
        &posting,
        "post",
        false,
        payload.path(),
        "",
        &[("RUN_HEAD_SHA", "f00df00df00df00df00df00df00df00df00df00d")],
    );
    assert_eq!(status, "skipped-clean");
    let log = posting.log();
    assert_eq!(dismissed(&log), ["101"]);
    assert!(!log.contains("comments"), "{log}");
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
    let gh = FakeGh::new(&["README.md"], None);
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
