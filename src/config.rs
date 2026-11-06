//! `.refquiem.toml` parameters and the files a scan leaves out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::error::CustomFailure;
use crate::sql::Dialect;
use crate::{ChangedPart, Error, Level};

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
    /// How a `--base` scan treats the items the range changed.
    #[serde(default)]
    pub(crate) changed: Changed,
    /// The dialect the SQL of files matching each gitignore-syntax glob, relative to the
    /// workspace root, is read in.
    #[serde(default)]
    pub(crate) sql_dialects: BTreeMap<String, Dialect>,
    /// Optional ambiguity vocabularies, English when the table is absent.
    vocabularies: Option<Vocabularies>,
}

/// The `[changed]` table.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Changed {
    /// Whether mentions of changed items are looked for, and whether they fail the run.
    #[serde(default)]
    pub(crate) level: Level,
    /// The parts of a definition compared.
    #[serde(default)]
    compare: Compare,
}

impl Changed {
    /// The last part of a definition compared, the parts before it compared too.
    pub(crate) fn compare(self) -> ChangedPart {
        match self.compare {
            Compare::Shapes => ChangedPart::Shape,
            Compare::Values => ChangedPart::Value,
            Compare::Bodies => ChangedPart::Body,
        }
    }
}

/// How much of a definition a `[changed]` table compares, each value including those before.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Compare {
    Shapes,
    #[default]
    Values,
    Bodies,
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
    #[serde(rename = "wasm@3.0")]
    Wasm3,
    #[serde(rename = "android@36")]
    Android36,
    #[serde(rename = "uikit@27.1")]
    Uikit27_1,
    #[serde(rename = "posix@2017")]
    Posix2017,
    #[serde(rename = "web-api@2026-10")]
    WebApi2026_10,
    #[serde(rename = "python@3.15")]
    Python3_15,
    #[serde(rename = "javascript@2026-10")]
    Javascript2026_10,
    #[serde(rename = "svg@2026-10")]
    Svg2026_10,
    #[serde(rename = "postgres@18")]
    Postgres18,
    #[serde(rename = "sqlite@3.54")]
    Sqlite3_54,
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
        self.excluded(file)
    }

    /// Whether an exclude glob matches `file`, from the repository root, whatever member the
    /// scan is limited to.
    pub(crate) fn excluded(&self, file: &str) -> bool {
        relative(&self.workspace, file).is_some_and(|relative| {
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

/// The `[sql-dialects]` globs, each matched alone so that a file two of them give different
/// dialects is caught.
pub(crate) struct SqlDialects {
    /// The workspace directory from the repository root.
    workspace: String,
    globs: Vec<(String, Dialect, Gitignore)>,
}

impl SqlDialects {
    pub(crate) fn new(
        config: &Config,
        workspace_dir: &Path,
        workspace: &str,
    ) -> Result<Self, Error> {
        let globs = config
            .sql_dialects
            .iter()
            .map(|(glob, &dialect)| {
                let mut builder = GitignoreBuilder::new(workspace_dir);
                builder
                    .add_line(None, glob)
                    .map_err(Error::SqlDialectGlob)?;
                let matcher = builder.build().map_err(Error::SqlDialectGlob)?;
                Ok((glob.clone(), dialect, matcher))
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            workspace: workspace.to_owned(),
            globs,
        })
    }

    /// The dialect the SQL of `file`, from the repository root, is read in, the one its globs
    /// give, else the one its nearest directory names, else the generic dialect.
    pub(crate) fn of(&self, file: &str) -> Result<Dialect, Error> {
        let mut given: Option<(&str, Dialect)> = None;
        if let Some(relative) = relative(&self.workspace, file) {
            for (glob, dialect, matcher) in &self.globs {
                if !matcher
                    .matched_path_or_any_parents(relative, false)
                    .is_ignore()
                {
                    continue;
                }
                match given {
                    Some((first, earlier)) if earlier != *dialect => {
                        return Err(Error::SqlDialects {
                            file: file.to_owned(),
                            first: first.to_owned(),
                            second: glob.clone(),
                        });
                    }
                    Some(_) => {}
                    None => given = Some((glob, *dialect)),
                }
            }
        }
        Ok(given
            .map(|(_, dialect)| dialect)
            .or_else(|| Dialect::of_path(file))
            .unwrap_or(Dialect::Generic))
    }
}

/// `file`, from the repository root, relative to the `workspace` directory, absent outside it.
fn relative<'f>(workspace: &str, file: &'f str) -> Option<&'f str> {
    if workspace.is_empty() {
        Some(file)
    } else {
        file.strip_prefix(workspace)
            .and_then(|rest| rest.strip_prefix('/'))
    }
}

/// Whether `file` lies inside directory `dir`, both from the repository root.
pub(crate) fn below(dir: &str, file: &str) -> bool {
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
