#![doc = include_str!("../README.md")]

use std::fmt::Write as _;
use std::path::PathBuf;

mod cache;
mod config;
mod deps;
mod entry;
mod error;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod git;
mod index;
mod intern;
mod manifest;
mod parse;
mod scan;
mod serde_names;
mod stdlib;
mod text;

pub use error::Error;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// One mention of a removed name.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Its mentions at head, by file and position.
    pub mentions: Vec<Mention>,
}

/// Every removed name still mentioned, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// One group per removed name.
    pub groups: Vec<Group>,
    /// Removed dependencies whose sources cargo could not resolve, so their items went
    /// unchecked.
    pub skipped: Vec<Skipped>,
    /// Names in `refquiem: allow(...)` markers that suppressed no mention.
    pub unused_markers: Vec<UnusedMarker>,
}

/// A name in a suppression marker that suppressed no mention.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
        let mentions: usize = self.groups.iter().map(|g| g.mentions.len()).sum();
        let mut out = format!(
            "refquiem: {} still mentioned, {}\n",
            plural(self.groups.len(), "removed name", "removed names"),
            plural(mentions, "mention", "mentions"),
        );
        for group in &self.groups {
            let _ = write!(out, "\n`{}` ({}", group.path, group.kind.describe());
            if let Some(member) = &group.member {
                let _ = write!(out, " of `{member}`");
            }
            out.push(')');
            let verb = if group.kind.is_manifest_entry() {
                "removed"
            } else {
                "deleted"
            };
            match &group.removed_in {
                Some(RemovedIn::Commit(id)) => {
                    let _ = write!(out, ", {verb} in {}", id.get(..7).unwrap_or(id));
                }
                Some(RemovedIn::WorkingTree) => {
                    let _ = write!(out, ", {verb} in the working tree");
                }
                None => {}
            }
            out.push('\n');
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
