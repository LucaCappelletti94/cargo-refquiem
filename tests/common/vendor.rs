//! Vendored registry crates, for tests whose fixtures depend on crates.io packages.

use crate::common::Repo;

impl Repo {
    /// Vendors library `name` at `version` with `lib` as its root, and replaces crates.io with
    /// the vendored crates so resolving dependencies never reaches the network.
    pub fn vendor(&self, name: &str, version: &str, lib: &str) {
        let dir = format!("vendor/{name}-{version}");
        self.write(
            &format!("{dir}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        );
        self.write(&format!("{dir}/src/lib.rs"), lib);
        self.write(&format!("{dir}/.cargo-checksum.json"), "{\"files\":{}}");
        self.write(
            ".cargo/config.toml",
            "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"vendor\"\n",
        );
    }
}
