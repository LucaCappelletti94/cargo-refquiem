//! The GitHub web root of a scanned repository and the URLs it gives a commit, a path and a line.

use std::borrow::Cow;

/// The uppercase hex digits of a percent encoding.
const HEX: &str = "0123456789ABCDEF";

/// The web root of a repository: `server` and `repository` when both are non-empty, else
/// `origin` when it names a GitHub repository.
#[must_use]
pub fn root(
    server: Option<&str>,
    repository: Option<&str>,
    origin: Option<&str>,
) -> Option<String> {
    if let (Some(server), Some(repository)) = (server, repository)
        && !server.is_empty()
        && !repository.is_empty()
    {
        return Some(format!("{server}/{repository}"));
    }
    from_remote(origin?)
}

/// The web root of the repository `origin` names when its host is `github.com`, `None` for any
/// other host or path.
#[must_use]
pub fn from_remote(origin: &str) -> Option<String> {
    match origin.split_once("://") {
        Some(("https" | "ssh", rest)) => {
            let rest = rest.rsplit('@').next()?;
            let (host, path) = rest.split_once('/')?;
            github(host, path)
        }
        _ => origin
            .split_once('@')
            .and_then(|(_, host_path)| host_path.split_once(':'))
            .and_then(|(host, path)| github(host, path)),
    }
}

/// `https://github.com/{owner}/{repo}` when `path` is exactly `owner/repo`, tolerating a
/// trailing `.git` and slash.
fn github(host: &str, path: &str) -> Option<String> {
    if host != "github.com" {
        return None;
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let repo = parts.next().filter(|part| !part.is_empty())?;
    parts
        .next()
        .is_none()
        .then(|| format!("https://github.com/{owner}/{repo}"))
}

/// The page of line `line` of `path` at `commit` under `root`, each path segment percent
/// encoded as jq's `@uri` encodes it.
#[must_use]
pub fn blob(root: &str, commit: &str, path: &str, line: usize) -> String {
    format!("{root}/blob/{commit}/{}#L{line}", encode_path(path))
}

/// The page of `commit` under `root`.
#[must_use]
pub fn commit_page(root: &str, commit: &str) -> String {
    format!("{root}/commit/{commit}")
}

/// `path` with each segment percent encoded as jq's `@uri` encodes it.
fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(&encode(segment));
    }
    out
}

/// One percent encoding of `segment` as jq's `@uri`, which leaves `A-Za-z0-9_.-~` alone.
fn encode(segment: &str) -> Cow<'_, str> {
    if segment.bytes().all(is_unreserved) {
        return Cow::Borrowed(segment);
    }
    let mut out = String::with_capacity(segment.len() + 2);
    for byte in segment.bytes() {
        if is_unreserved(byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX.as_bytes()[usize::from(byte >> 4)]));
            out.push(char::from(HEX.as_bytes()[usize::from(byte & 15)]));
        }
    }
    Cow::Owned(out)
}

/// Whether `@uri` leaves `byte` alone.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_repository_wins_over_the_origin() {
        assert_eq!(
            root(
                Some("https://github.com"),
                Some("o/r"),
                Some("git@github.com:x/y.git")
            ),
            Some("https://github.com/o/r".to_owned())
        );
    }

    #[test]
    fn an_absent_or_empty_environment_falls_back_to_the_origin() {
        for (server, repository) in [(None, None), (Some(""), Some("o/r")), (None, Some("o/r"))] {
            assert_eq!(
                root(server, repository, Some("git@github.com:x/y.git")),
                Some("https://github.com/x/y".to_owned()),
                "{server:?}, {repository:?}"
            );
        }
    }

    #[test]
    fn a_github_origin_gives_the_web_root() {
        for origin in [
            "https://github.com/o/r",
            "https://github.com/o/r.git",
            "https://github.com/o/r/",
            "https://user:pass@github.com/o/r.git",
            "ssh://git@github.com/o/r",
            "ssh://git@github.com/o/r.git",
            "git@github.com:o/r",
            "git@github.com:o/r.git",
        ] {
            assert_eq!(
                from_remote(origin),
                Some("https://github.com/o/r".to_owned()),
                "{origin}"
            );
        }
    }

    #[test]
    fn another_origin_gives_no_web_root() {
        for origin in [
            "https://gitlab.com/o/r",
            "git@gitlab.com:o/r.git",
            "ssh://git@gitlab.com/o/r",
            "https://example.com/o/r",
            "https://github.com:443/o/r",
            "https://github.com/o",
            "https://github.com/o/r/sub",
            "https://github.com/",
            "not a url",
        ] {
            assert_eq!(from_remote(origin), None, "{origin}");
        }
    }

    #[test]
    fn a_blob_url_encodes_each_path_segment() {
        assert_eq!(
            blob("https://github.com/o/r", "abc123", "src/a b/c#d.md", 12),
            "https://github.com/o/r/blob/abc123/src/a%20b/c%23d.md#L12"
        );
        assert_eq!(
            blob("https://github.com/o/r", "abc123", "src/lib.rs", 1),
            "https://github.com/o/r/blob/abc123/src/lib.rs#L1"
        );
    }

    #[test]
    fn a_commit_url_names_the_commit() {
        assert_eq!(
            commit_page("https://github.com/o/r", "abc123"),
            "https://github.com/o/r/commit/abc123"
        );
    }
}
