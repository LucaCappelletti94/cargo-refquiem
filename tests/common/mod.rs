//! Throwaway git repositories for end-to-end tests.

use std::path::Path;
use std::process::Command;

/// A temporary git repository with an isolated configuration.
pub struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    /// An empty repository on branch `main` with a `fixture` package manifest staged.
    pub fn new() -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        repo.write(".gitignore", "/target\n");
        repo
    }

    /// The repository root.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Writes `contents` to `relative`, creating parent directories.
    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// Stages everything and commits, returning the full commit id.
    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.rev("HEAD")
    }

    /// Resolves a revision to a full commit id.
    pub fn rev(&self, spec: &str) -> String {
        self.git(&["rev-parse", spec]).trim().to_owned()
    }

    /// Runs git in the repository and returns its stdout.
    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.com",
            ])
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(self.dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}
