//! `.refquiem.toml` parameters and the files a scan leaves out.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::Error;
use crate::error::CustomFailure;

/// The file name looked up at the workspace root.
pub(crate) const FILE_NAME: &str = ".refquiem.toml";

/// Excludes every scan applies before the configured ones.
const DEFAULT_EXCLUDES: [&str; 8] = [
    "CHANGELOG*",
    "changelog/",
    "RELEASES*",
    "RELEASE-NOTES*",
    "RELEASE_NOTES*",
    "NEWS*",
    "HISTORY*",
    "CHANGES*",
];

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
    /// Whether comments holding nothing but Rust code are scanned for mentions anyway.
    #[serde(default)]
    pub(crate) scan_commented_code: bool,
    /// The revision a scan of the full history starts at instead.
    pub(crate) since: Option<String>,
    /// Optional ambiguity vocabularies, English when the table is absent.
    vocabularies: Option<Vocabularies>,
}

impl Config {
    /// Whether the selection includes `builtin`, English alone when the table is absent.
    pub(crate) fn selects(&self, builtin: Builtin) -> bool {
        match &self.vocabularies {
            Some(vocabularies) => vocabularies.builtins.contains(&builtin),
            None => builtin == Builtin::English,
        }
    }

    /// The custom vocabulary files the selection names, empty without a table.
    pub(crate) fn custom_files(&self) -> &[String] {
        match &self.vocabularies {
            Some(vocabularies) => &vocabularies.files,
            None => &[],
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Vocabularies {
    #[serde(default)]
    builtins: Vec<Builtin>,
    /// Custom vocabulary files, resolved against the directory of this config.
    #[serde(default)]
    files: Vec<String>,
}

/// A bundled vocabulary as configuration names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub(crate) enum Builtin {
    #[serde(rename = "english")]
    English,
    #[serde(rename = "sql@2023")]
    Sql2023,
    #[serde(rename = "wasi@preview1")]
    WasiPreview1,
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

/// The category of a custom vocabulary file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Category {
    /// Words claim bare names in running text.
    NaturalLanguage,
    /// Words claim bare names in running text and inline code spans.
    Technical,
}

impl Category {
    /// The category as its configuration spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Category::NaturalLanguage => "natural-language",
            Category::Technical => "technical",
        }
    }
}

/// The case rule of a custom vocabulary file.
#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Case {
    /// Words match their exact spelling.
    Sensitive,
    /// Words match with ASCII case ignored.
    AsciiInsensitive,
    /// Words match with Unicode lowercase applied.
    UnicodeLowercase,
}

impl Case {
    /// The rule as its configuration spelling.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Case::Sensitive => "sensitive",
            Case::AsciiInsensitive => "ascii-insensitive",
            Case::UnicodeLowercase => "unicode-lowercase",
        }
    }
}

/// The document of a custom vocabulary file.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CustomFile {
    format_version: u32,
    kind: Category,
    case: Case,
    words: Vec<String>,
}

/// A selected custom vocabulary file, read and validated.
pub(crate) struct Custom {
    /// The canonical path the selection resolved to.
    pub(crate) path: PathBuf,
    pub(crate) kind: Category,
    pub(crate) case: Case,
    pub(crate) format_version: u32,
    pub(crate) words: Vec<String>,
}

/// The distinct custom vocabulary files the config selects, in canonical path order,
/// each read once.
pub(crate) fn load_custom(config_path: &Path, config: &Config) -> Result<Vec<Custom>, Error> {
    let base = config_path.parent().unwrap_or(Path::new("."));
    let mut paths = BTreeSet::new();
    for entry in config.custom_files() {
        paths.insert(canonical(&base.join(entry))?);
    }
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        files.push(load_custom_file(path)?);
    }
    Ok(files)
}

/// The custom vocabulary at `path`, a UTF-8 `TOML` document with supported values.
fn load_custom_file(path: PathBuf) -> Result<Custom, Error> {
    let bytes =
        std::fs::read(&path).map_err(|source| custom_error(&path, CustomFailure::Io(source)))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| custom_error(&path, CustomFailure::Utf8))?;
    let mut file: CustomFile = toml::from_str(text)
        .map_err(|source| custom_error(&path, CustomFailure::Toml(Box::new(source))))?;
    if file.format_version != 1 {
        return Err(custom_error(
            &path,
            CustomFailure::Version(file.format_version),
        ));
    }
    if let Some(at) = file.words.iter().position(|word| {
        word.is_empty() || word.chars().any(|c| c.is_whitespace() || c.is_control())
    }) {
        let word = file.words.swap_remove(at);
        return Err(custom_error(&path, CustomFailure::Word(word)));
    }
    Ok(Custom {
        path,
        kind: file.kind,
        case: file.case,
        format_version: file.format_version,
        words: file.words,
    })
}

fn custom_error(path: &Path, source: CustomFailure) -> Error {
    Error::Custom {
        path: path.to_path_buf(),
        source: Box::new(source),
    }
}

/// `path` after symlink resolution, an error with the path when it cannot be resolved.
fn canonical(path: &Path) -> Result<PathBuf, Error> {
    path.canonicalize()
        .map_err(|source| custom_error(path, CustomFailure::Io(source)))
}

/// Which files a scan reads for mentions.
pub(crate) struct Filter {
    excludes: Gitignore,
    /// The workspace directory from the repository root.
    workspace: String,
    generated_markers: Vec<String>,
    scan_generated: bool,
    /// Whether comments holding nothing but Rust code are scanned.
    pub(crate) scan_commented_code: bool,
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
            scan_commented_code: config.scan_commented_code,
            package_dir,
        })
    }

    /// Whether `file`, from the repository root, is left out before it is read, exclude globs
    /// matching only below the workspace.
    pub(crate) fn skips_path(&self, file: &str) -> bool {
        if let Some(dir) = &self.package_dir
            && !below(dir, file)
        {
            return true;
        }
        let relative = if self.workspace.is_empty() {
            Some(file)
        } else {
            file.strip_prefix(&self.workspace)
                .and_then(|rest| rest.strip_prefix('/'))
        };
        relative.is_some_and(|relative| {
            self.excludes
                .matched_path_or_any_parents(relative, false)
                .is_ignore()
        })
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
