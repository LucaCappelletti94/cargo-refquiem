//! `.refquiem.toml` parameters and the files a scan leaves out.

use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::Error;

/// The file name looked up at the workspace root.
pub(crate) const FILE_NAME: &str = ".refquiem.toml";

/// Excludes every scan applies before the configured ones.
const DEFAULT_EXCLUDES: [&str; 2] = ["CHANGELOG*", "changelog/"];

/// Parameters from a config file.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Config {
    /// Gitignore-syntax globs, relative to the workspace root, of files not scanned for mentions.
    #[serde(default)]
    pub(crate) exclude: Vec<String>,
    /// Paths and names never reported.
    #[serde(default)]
    pub(crate) allow: Vec<String>,
    /// More phrases that mark a file generated in its first lines.
    #[serde(default)]
    pub(crate) generated_markers: Vec<String>,
    /// Whether generated files are scanned for mentions anyway.
    #[serde(default)]
    pub(crate) scan_generated: bool,
    /// The revision a scan of the full history starts at instead.
    pub(crate) since: Option<String>,
}

/// The config at `path`, the default one when `path` is absent and not `required`.
pub(crate) fn load(path: &Path, required: bool) -> Result<Config, Error> {
    if !required && !path.exists() {
        return Ok(Config::default());
    }
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::Config {
        path: path.to_path_buf(),
        source: Box::new(source),
    })
}

/// Which files a scan reads for mentions.
pub(crate) struct Filter {
    excludes: Gitignore,
    /// The workspace directory from the repository root.
    workspace: String,
    generated_markers: Vec<String>,
    scan_generated: bool,
    /// The only member directory scanned, from the repository root.
    package_dir: Option<String>,
}

impl Filter {
    pub(crate) fn new(
        config: &Config,
        workspace_dir: &Path,
        workspace: &str,
        package_dir: Option<String>,
    ) -> Result<Self, Error> {
        let mut builder = GitignoreBuilder::new(workspace_dir);
        for glob in DEFAULT_EXCLUDES
            .iter()
            .copied()
            .chain(config.exclude.iter().map(String::as_str))
        {
            builder.add_line(None, glob).map_err(Error::Exclude)?;
        }
        let excludes = builder.build().map_err(Error::Exclude)?;
        Ok(Self {
            excludes,
            workspace: workspace.to_owned(),
            generated_markers: config.generated_markers.clone(),
            scan_generated: config.scan_generated,
            package_dir,
        })
    }

    /// Whether `file`, from the repository root, is left out before it is read.
    pub(crate) fn skips_path(&self, file: &str) -> bool {
        if let Some(dir) = &self.package_dir
            && !below(dir, file)
        {
            return true;
        }
        let relative = file
            .strip_prefix(&self.workspace)
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|_| !self.workspace.is_empty())
            .unwrap_or(file);
        self.excludes
            .matched_path_or_any_parents(relative, false)
            .is_ignore()
    }

    /// Whether a file holding `text` is left out as generated.
    pub(crate) fn skips_text(&self, text: &str) -> bool {
        !self.scan_generated && is_generated(text, &self.generated_markers)
    }
}

/// Whether `file` lies inside directory `dir`, both from the repository root.
fn below(dir: &str, file: &str) -> bool {
    dir.is_empty()
        || file
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `text` declares itself generated in its first five lines, where rustfmt looks.
fn is_generated(text: &str, markers: &[String]) -> bool {
    text.lines().take(5).any(|line| {
        line.contains("@generated")
            || (line.starts_with("// Code generated ") && line.ends_with(" DO NOT EDIT."))
            || markers.iter().any(|marker| line.contains(marker.as_str()))
    })
}
