//! The offline lockfile, for tests whose dependencies must resolve without the network.

use std::process::Command;

use crate::common::Repo;

impl Repo {
    /// Writes `Cargo.lock` without touching the network, as every dependency is a path
    /// dependency or vendored.
    pub fn lock(&self) {
        let status = Command::new("cargo")
            .args(["generate-lockfile", "--offline", "--quiet"])
            .current_dir(self.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
