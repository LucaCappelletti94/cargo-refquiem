//! Package features removed between two commits, for tests of vanished features.

use std::fmt::Write as _;

use crate::common::{PACKAGE, Repo};

impl Repo {
    /// Declares `names` as empty features in the package manifest with an empty library,
    /// commits with `message`, and removes the features from the manifest.
    pub fn gone_features(&self, names: &[&str], message: &str) {
        let mut manifest = format!("{PACKAGE}\n[features]\n");
        for name in names {
            let _ = writeln!(manifest, "{name} = []");
        }
        self.write("src/lib.rs", "");
        self.write("Cargo.toml", &manifest);
        self.commit(message);
        self.write("Cargo.toml", PACKAGE);
    }
}
