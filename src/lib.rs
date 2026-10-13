#![doc = include_str!("../README.md")]

use std::fmt::Write as _;
use std::path::PathBuf;

mod cache;
mod error;
mod git;
mod index;
mod intern;
mod manifest;
mod parse;
mod scan;
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
    /// The commit range.
    pub range: Range,
    /// Where parsed files are cached between runs, `target/refquiem` of the workspace when absent.
    pub cache_dir: Option<PathBuf>,
}

impl Options {
    /// The full history of the repository containing `root`.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            range: Range::Full,
            cache_dir: None,
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
        }
    }
}

/// Where a mention sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// A `//` or `/* */` comment.
    Comment,
    /// A doc comment.
    DocComment,
    /// A Markdown file.
    Markdown,
}

impl Context {
    /// The context in words.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::DocComment => "doc comment",
            Self::Markdown => "Markdown",
        }
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
    /// The full path of the removed name.
    pub path: String,
    /// Its kind.
    pub kind: ItemKind,
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
}

impl Report {
    /// Whether nothing stale was found.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.groups.is_empty()
    }

    /// The report as printed by `cargo refquiem`.
    #[must_use]
    pub fn human(&self) -> String {
        if self.is_clean() {
            return "refquiem: no stale mentions\n".to_owned();
        }
        let mentions: usize = self.groups.iter().map(|g| g.mentions.len()).sum();
        let mut out = format!(
            "refquiem: {} still mentioned, {}\n",
            plural(self.groups.len(), "removed name", "removed names"),
            plural(mentions, "mention", "mentions"),
        );
        for group in &self.groups {
            let _ = write!(out, "\n`{}` ({})", group.path, group.kind.describe());
            match &group.removed_in {
                Some(RemovedIn::Commit(id)) => {
                    let _ = write!(out, ", deleted in {}", id.get(..7).unwrap_or(id));
                }
                Some(RemovedIn::WorkingTree) => out.push_str(", deleted in the working tree"),
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

/// Scans the repository and workspace containing `options.root`.
///
/// # Errors
///
/// Fails when no git repository or `Cargo.toml` contains `options.root`, a revision does not
/// resolve, git objects or working tree files cannot be read, or a manifest or Rust file of
/// head does not parse.
pub fn run(options: &Options) -> Result<Report, Error> {
    scan::run(options)
}
