#![doc = include_str!("../README.md")]

use std::fmt::Write as _;
use std::path::PathBuf;

mod cache;
mod config;
mod deps;
mod entry;
mod error;
mod expand;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod git;
mod index;
mod intern;
mod manifest;
mod output;
mod parse;
mod scan;
mod serde_names;
mod stdlib;
mod suggest;
mod text;

pub use error::Error;

/// The hash maps and sets the internal tables keep, which no public type holds.
pub(crate) use hashbrown::{HashMap, HashSet};

/// Which commits are scanned and what they are compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    /// Every commit reachable from `HEAD`, compared against the working tree.
    Full,
    /// The revision and every commit after it up to `HEAD`, compared against the working tree.
    Since(String),
    /// The base revision plus every commit in `base..head`, compared against `head` or the working tree.
    Base {
        /// The revision the range starts at.
        base: String,
        /// The compared revision, the working tree when absent.
        head: Option<String>,
    },
}

/// What to scan.
#[derive(Debug, Clone)]
pub struct Options {
    /// A directory inside the repository and the workspace.
    pub root: PathBuf,
    /// The commit range, `Full` taking the config file's `since` when it has one.
    pub range: Range,
    /// Where parsed files are cached between runs and the manifests that locate dependency
    /// sources are written, `target/refquiem` of the workspace when absent.
    pub cache_dir: Option<PathBuf>,
    /// The config file read instead of the workspace's `.refquiem.toml`.
    pub config: Option<PathBuf>,
    /// The only workspace member whose files are scanned for mentions.
    pub package: Option<String>,
}

impl Options {
    /// The full history of the repository containing `root`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            range: Range::Full,
            cache_dir: None,
            config: None,
            package: None,
        }
    }
}

/// The kind of a removed name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// A module.
    Module,
    /// A struct.
    Struct,
    /// An enum.
    Enum,
    /// A union.
    Union,
    /// A trait.
    Trait,
    /// A trait alias.
    TraitAlias,
    /// A type alias or foreign type.
    TypeAlias,
    /// A constant.
    Const,
    /// A static.
    Static,
    /// A free function.
    Function,
    /// A `macro_rules!` macro.
    Macro,
    /// An enum variant.
    Variant,
    /// A named field.
    Field,
    /// An associated function of a type or trait.
    Method,
    /// An associated constant.
    AssociatedConst,
    /// An associated type.
    AssociatedType,
    /// A name created by `use`.
    Reexport,
    /// A workspace member package.
    Package,
    /// A dependency of a workspace member, by key or by renamed package name.
    Dependency,
    /// A Cargo feature of a workspace member.
    Feature,
    /// A binary target.
    Binary,
    /// An example target.
    Example,
    /// A serde serialized spelling of a field or variant.
    Serialized,
}

impl ItemKind {
    /// The kind in words.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Union => "union",
            Self::Trait => "trait",
            Self::TraitAlias => "trait alias",
            Self::TypeAlias => "type alias",
            Self::Const => "const",
            Self::Static => "static",
            Self::Function => "function",
            Self::Macro => "macro",
            Self::Variant => "variant",
            Self::Field => "field",
            Self::Method => "method",
            Self::AssociatedConst => "associated const",
            Self::AssociatedType => "associated type",
            Self::Reexport => "re-export",
            Self::Package => "package",
            Self::Dependency => "dependency",
            Self::Feature => "feature",
            Self::Binary => "binary",
            Self::Example => "example",
            Self::Serialized => "serialized name",
        }
    }

    /// Whether this kind names a manifest entry rather than an item of the code.
    #[must_use]
    pub fn is_manifest_entry(self) -> bool {
        matches!(
            self,
            Self::Package | Self::Dependency | Self::Feature | Self::Binary | Self::Example
        )
    }
}

/// The text a mention sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A `//` or `/* */` comment, read as plain text.
    Comment,
    /// A doc comment, read as Markdown.
    DocComment,
    /// A Markdown file.
    Markdown,
    /// A string literal in Rust code.
    StringLiteral,
}

/// The Markdown structure a mention sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Running text.
    Text,
    /// An inline code span.
    CodeSpan,
    /// A code block, compiled when rustdoc runs it as a doctest.
    CodeBlock {
        /// Whether rustdoc compiles the block.
        compiled: bool,
    },
    /// A link destination, an intra-doc link or a rustdoc page.
    Link,
}

/// Where a mention sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Context {
    /// The text holding the mention.
    pub source: Source,
    /// Its structure within that text.
    pub kind: Kind,
}

impl Context {
    /// The context in words.
    #[must_use]
    pub fn describe(self) -> String {
        let source = match self.source {
            Source::Comment => "comment",
            Source::DocComment => "doc comment",
            Source::Markdown => "Markdown",
            Source::StringLiteral => "string literal",
        };
        let kind = match self.kind {
            Kind::Text => "",
            Kind::CodeSpan => " code span",
            Kind::CodeBlock { compiled: true } => " code block",
            Kind::CodeBlock { compiled: false } => " uncompiled code block",
            Kind::Link => " link",
        };
        format!("{source}{kind}")
    }
}

/// The change that removed a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovedIn {
    /// A commit, by full id.
    Commit(String),
    /// Uncommitted changes.
    WorkingTree,
}

/// A commit id, or `working_tree`.
impl serde::Serialize for RemovedIn {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Commit(id) => serializer.serialize_str(id),
            Self::WorkingTree => serializer.serialize_str("working_tree"),
        }
    }
}

/// One mention of a removed name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Mention {
    /// The file, relative to the repository root with `/` separators.
    pub file: String,
    /// One-based line.
    pub line: usize,
    /// One-based column in characters.
    pub column: usize,
    /// The path as written.
    pub text: String,
    /// Where it sits.
    pub context: Context,
    /// The mention's line with every suggested rewrite on that line applied.
    pub replacement: Option<String>,
}

/// How a suggested name relates to the removed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The same name and kind under another parent.
    Moved,
    /// A near name of the same kind and parent.
    Renamed,
    /// The only new sibling with the same signature, or the only crate added to the same table.
    Replaced,
}

impl Change {
    /// The words that introduce the suggested name.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Moved => "moved to",
            Self::Renamed => "renamed to",
            Self::Replaced => "replaced by",
        }
    }
}

/// The name a removed name most likely became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Its full path, or the name of a manifest entry.
    pub path: String,
    /// How it relates to the removed name.
    pub change: Change,
    /// Where head declares it, absent when no file of head declares it.
    pub declared_at: Option<Position>,
}

/// A line of a file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Position {
    /// The file, relative to the repository root with `/` separators.
    pub file: String,
    /// One-based line.
    pub line: usize,
}

/// A removed name and its remaining mentions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The full path of a removed item, or the name of a removed manifest entry.
    pub path: String,
    /// Its kind.
    pub kind: ItemKind,
    /// For a manifest entry in a workspace of several members, the package it belonged to.
    pub member: Option<String>,
    /// The change that removed it, unknown when no commit of the range shows the removal.
    pub removed_in: Option<RemovedIn>,
    /// Where the range's starting revision declares it, absent for a full-history scan or when
    /// that revision has no declaration of it in a file.
    pub declared_at: Option<Position>,
    /// The name it most likely became.
    pub suggestion: Option<Suggestion>,
    /// The names it may have become when no single one stands out.
    pub candidates: Vec<String>,
    /// Its mentions at head, by file and position.
    pub mentions: Vec<Mention>,
}

/// The fields, the suggestion as its path and its position as `suggested_at`, plus the
/// `headline` a reader shows for the group.
impl serde::Serialize for Group {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;
        let mut group = serializer.serialize_struct("Group", 10)?;
        group.serialize_field("path", &self.path)?;
        group.serialize_field("kind", &self.kind)?;
        group.serialize_field("member", &self.member)?;
        group.serialize_field("removed_in", &self.removed_in)?;
        group.serialize_field("declared_at", &self.declared_at)?;
        group.serialize_field("suggestion", &self.suggestion.as_ref().map(|s| &s.path))?;
        group.serialize_field(
            "suggested_at",
            &self
                .suggestion
                .as_ref()
                .and_then(|s| s.declared_at.as_ref()),
        )?;
        group.serialize_field("candidates", &self.candidates)?;
        group.serialize_field("headline", &self.headline())?;
        group.serialize_field("mentions", &self.mentions)?;
        group.end()
    }
}

/// Every removed name still mentioned, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    /// The commit the range starts from, whose files the `declared_at` positions refer to,
    /// absent for a full-history scan.
    pub base: Option<String>,
    /// One group per removed name.
    pub groups: Vec<Group>,
    /// Removed dependencies whose sources cargo could not resolve, so their items went
    /// unchecked.
    pub skipped: Vec<Skipped>,
    /// Names in `refquiem: allow(...)` markers that suppressed no mention.
    pub unused_markers: Vec<UnusedMarker>,
}

/// A name in a suppression marker that suppressed no mention.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UnusedMarker {
    /// The file holding the marker, from the repository root.
    pub file: String,
    /// One-based line of the marker.
    pub line: usize,
    /// One-based character column of the name.
    pub column: usize,
    /// The name as written in the marker.
    pub name: String,
}

/// A removed dependency whose items could not be checked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Skipped {
    /// The dependency key.
    pub dependency: String,
    /// Why cargo could not resolve it.
    pub reason: String,
}

impl Report {
    /// Whether nothing stale was found and every suppression marker suppressed something.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.groups.is_empty() && self.unused_markers.is_empty()
    }

    /// The report as printed by `cargo refquiem`.
    #[must_use]
    pub fn human(&self) -> String {
        let mut out = self.findings();
        for marker in &self.unused_markers {
            let _ = writeln!(
                out,
                "{}:{}:{}  refquiem: allow({}) suppresses nothing",
                marker.file, marker.line, marker.column, marker.name
            );
        }
        for skipped in &self.skipped {
            let reason = skipped
                .reason
                .lines()
                .find(|line| line.starts_with("error:"))
                .or_else(|| skipped.reason.lines().next())
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "warning: removed dependency `{}` could not be resolved, so its items were not checked: {reason}",
                skipped.dependency
            );
        }
        out
    }

    fn findings(&self) -> String {
        if self.groups.is_empty() {
            return "refquiem: no stale mentions\n".to_owned();
        }
        let mut out = format!("refquiem: {}\n", self.tally());
        for group in &self.groups {
            let _ = writeln!(out, "\n{}", group.headline());
            for mention in &group.mentions {
                let _ = writeln!(
                    out,
                    "  {}:{}:{}  {}",
                    mention.file,
                    mention.line,
                    mention.column,
                    mention.context.describe()
                );
            }
        }
        out
    }

    /// How many removed names are still mentioned, and how often.
    fn tally(&self) -> String {
        let mentions: usize = self.groups.iter().map(|g| g.mentions.len()).sum();
        format!(
            "{} still mentioned, {}",
            plural(self.groups.len(), "removed name", "removed names"),
            plural(mentions, "mention", "mentions"),
        )
    }
}

impl Group {
    /// The removed name with its kind, member, the change that removed it and what it became.
    fn headline(&self) -> String {
        let mut out = format!("`{}` ({}", self.path, self.kind.describe());
        if let Some(member) = &self.member {
            let _ = write!(out, " of `{member}`");
        }
        out.push(')');
        let verb = if self.kind.is_manifest_entry() {
            "removed"
        } else {
            "deleted"
        };
        match &self.removed_in {
            Some(RemovedIn::Commit(id)) => {
                let _ = write!(out, ", {verb} in {}", id.get(..7).unwrap_or(id));
            }
            Some(RemovedIn::WorkingTree) => {
                let _ = write!(out, ", {verb} in the working tree");
            }
            None => {}
        }
        if let Some(became) = self.became() {
            let _ = write!(out, ", {became}");
        }
        out
    }

    /// What the removed name became, as `renamed to X` or `perhaps X or Y`.
    fn became(&self) -> Option<String> {
        if let Some(suggestion) = &self.suggestion {
            return Some(format!(
                "{} `{}`",
                suggestion.change.describe(),
                suggestion.path
            ));
        }
        let (last, rest) = self.candidates.split_last()?;
        let rest: Vec<String> = rest.iter().map(|c| format!("`{c}`")).collect();
        Some(format!("perhaps {} or `{last}`", rest.join(", ")))
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The scan thread's stack, as `syn` leaves deep nesting to the stack (<https://github.com/dtolnay/syn/issues/901>).
const SCAN_STACK: usize = 256 << 20;

/// Scans the repository and workspace containing `options.root`.
///
/// # Errors
///
/// Fails when repository data cannot be read, a Markdown reader fails, or the scan thread cannot start.
pub fn run(options: &Options) -> Result<Report, Error> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("refquiem".to_owned())
            .stack_size(SCAN_STACK)
            .spawn_scoped(scope, || scan::run(options))
            .map_err(Error::Thread)?
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
