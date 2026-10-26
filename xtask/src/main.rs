//! Repository maintenance tasks, run with `cargo xtask <task>`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rustdoc_types::{Crate, Id, Item, ItemEnum, ItemKind, StructKind, Type, Visibility};

/// The nightly whose `rust-docs-json` the standard library layers are built from, shared with CI.
/// Its rustdoc JSON format must match the `rustdoc-types` version this crate depends on.
const TOOLCHAIN: &str = include_str!("../../data/std/TOOLCHAIN").trim_ascii();

/// The standard library crates, each one a layer keyed by the root its paths start with.
const LAYERS: [&str; 4] = ["core", "alloc", "std", "proc_macro"];

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("usage: cargo xtask vocab")]
    Usage,
    #[error("running rustc +{TOOLCHAIN} --print sysroot")]
    Rustc(#[source] std::io::Error),
    #[error(
        "rustc +{TOOLCHAIN} failed, install it with: rustup toolchain install {TOOLCHAIN} --component rust-docs-json"
    )]
    Toolchain,
    #[error("reading {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parsing {path}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path} has rustdoc JSON format {found}, rustdoc-types expects {expected}")]
    Format {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error("building the path set")]
    Fst(#[from] fst::Error),
    #[error("writing {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("running {program}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{program} failed: {stderr}")]
    Failed { program: String, stderr: String },
}

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("vocab") => vocab(),
        _ => Err(Error::Usage),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut message = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                message.push_str(": ");
                message.push_str(&cause.to_string());
                source = cause.source();
            }
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the standard library layers and the English, SQL, WASI, WebAssembly and Android
/// word lists under `data/`.
fn vocab() -> Result<(), Error> {
    let data = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("data");
    standard_library(&data.join("std"))?;
    english(&data.join("english"))?;
    sql(&data.join("sql"))?;
    wasi(&data.join("wasi"))?;
    wasm(&data.join("wasm"))?;
    android(&data.join("android"))
}

/// Writes `<layer>.fst`, every suffix of two or more segments of every public path reachable
/// from the layer's crate root, and `<layer>-names.fst`, every segment of those paths.
fn standard_library(out: &Path) -> Result<(), Error> {
    let json = sysroot()?.join("share/doc/rust/json");
    let mut crates = Vec::new();
    for name in LAYERS {
        crates.push(load(&json.join(format!("{name}.json")))?);
    }
    let library = Library::new(&crates);
    create_dir(out)?;
    for (layer, name) in LAYERS.iter().enumerate() {
        let mut paths = Paths::default();
        let root = library.crates[layer].root;
        library.module(layer, root, &mut vec![(*name).to_owned()], &mut paths);
        let mut suffixes = BTreeSet::new();
        let mut names = BTreeSet::new();
        for path in &paths.found {
            for start in 0..path.len().saturating_sub(1) {
                suffixes.insert(path[start..].join("::"));
            }
            names.extend(path.iter().map(String::as_str));
        }
        let suffix_count = write_set(
            &out.join(format!("{name}.fst")),
            suffixes.iter().map(String::as_str),
        )?;
        let name_count = write_set(&out.join(format!("{name}-names.fst")), names)?;
        println!(
            "{name}: {} paths, {suffix_count} suffixes, {name_count} names",
            paths.found.len()
        );
    }
    Ok(())
}

/// Writes `words.fst`, the lower-cased ESDB size 70 word list in every English dialect, and
/// ESDB's `Copyright`, which its license requires next to any copy.
fn english(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/english/ESDB").trim_ascii();
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("target")
        .join("xtask")
        .join(format!("esdb-{pinned}"));
    if !checkout.join("scowl.db").is_file() {
        create_dir(&checkout)?;
        run(&checkout, "git", &["init", "-q"])?;
        run(
            &checkout,
            "git",
            &["fetch", "-q", "--depth", "1", ESDB_REPOSITORY, pinned],
        )?;
        run(&checkout, "git", &["checkout", "-q", "FETCH_HEAD"])?;
        run(&checkout, "make", &["scowl.db"])?;
    }
    let listing = run(
        &checkout,
        "./scowl",
        &[
            "--db",
            "scowl.db",
            "word-list",
            "--hyphen",
            "70",
            "A,B,Z,C",
            "1",
        ],
    )?;
    // Letters, and hyphenated compounds such as `well-known`, the shape a kebab-case
    // serialized spelling shares with prose.
    let words: BTreeSet<String> = listing
        .lines()
        .filter(|word| {
            word.split('-')
                .all(|part| !part.is_empty() && part.chars().all(char::is_alphabetic))
        })
        .map(str::to_lowercase)
        .collect();
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let copyright = checkout.join("Copyright");
    let target = out.join("Copyright");
    std::fs::copy(&copyright, &target).map_err(|source| Error::Write {
        path: target,
        source,
    })?;
    println!("english: {count} words");
    Ok(())
}

/// Where ESDB, the English Speller Database, is published.
const ESDB_REPOSITORY: &str = "https://github.com/en-wl/wordlist";

/// Where PostgreSQL, whose documentation tabulates the SQL standard's key words, is published.
const POSTGRESQL_REPOSITORY: &str = "https://github.com/postgres/postgres";

/// The SQL:2023 key-word tables of parts 2, 9 and 14 that PostgreSQL's documentation carries.
const SQL_2023_RESERVED: [&str; 3] = [
    "doc/src/sgml/keywords/sql2023-02-reserved.txt",
    "doc/src/sgml/keywords/sql2023-09-reserved.txt",
    "doc/src/sgml/keywords/sql2023-14-reserved.txt",
];

/// Where WASI, the WebAssembly System Interface, is published.
const WASI_REPOSITORY: &str = "https://github.com/WebAssembly/WASI";

/// The Preview 1 interface definition, whose `export` names are its functions.
const WASI_PREVIEW1: &str = "legacy/preview1/witx/wasi_snapshot_preview1.witx";

/// Where the WebAssembly specification is published.
const WASM_REPOSITORY: &str = "https://github.com/WebAssembly/spec";

/// Where AOSP publishes the Android framework, whose public API it describes in signature files.
const AOSP_FRAMEWORKS_BASE: &str = "https://android.googlesource.com/platform/frameworks/base";

/// A reader of the files of `repository` at tag `pinned`, through a partial clone whose
/// blobs arrive on demand, so only the files read are downloaded.
fn pinned_files(
    repository: &str,
    name: &str,
    pinned: &str,
) -> Result<impl Fn(&str) -> Result<String, Error>, Error> {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("target")
        .join("xtask")
        .join(format!("{name}-{pinned}"));
    if !checkout.join(".git").is_dir() {
        create_dir(&checkout)?;
        run(&checkout, "git", &["init", "-q"])?;
        run(&checkout, "git", &["remote", "add", "origin", repository])?;
    }
    let tag = format!("refs/tags/{pinned}");
    run(
        &checkout,
        "git",
        &[
            "fetch",
            "-q",
            "--depth",
            "1",
            "--filter=blob:none",
            "origin",
            &format!("+{tag}:{tag}"),
        ],
    )?;
    Ok(move |path: &str| run(&checkout, "git", &["show", &format!("{tag}:{path}")]))
}

/// Writes `words.fst`, the reserved key words of SQL:2023 in their uppercase spelling, and
/// PostgreSQL's `COPYRIGHT`, which its license requires next to any copy.
fn sql(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/sql/POSTGRESQL").trim_ascii();
    let show = pinned_files(POSTGRESQL_REPOSITORY, "postgresql", pinned)?;
    let mut words = BTreeSet::new();
    for table in SQL_2023_RESERVED {
        words.extend(
            show(table)?
                .lines()
                .map(str::trim_ascii)
                .filter(|word| !word.is_empty())
                .map(str::to_owned),
        );
    }
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let target = out.join("COPYRIGHT");
    std::fs::write(&target, show("COPYRIGHT")?).map_err(|source| Error::Write {
        path: target,
        source,
    })?;
    println!("sql@2023: {count} words");
    Ok(())
}

/// Writes `words.fst`, the function names of WASI Preview 1, and WASI's `LICENSE.md`, the
/// attribution its contributor agreement asks of a copy.
fn wasi(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/wasi/WASI").trim_ascii();
    let show = pinned_files(WASI_REPOSITORY, "wasi", pinned)?;
    let witx = show(WASI_PREVIEW1)?;
    let words: BTreeSet<&str> = witx
        .split("(@interface func (export \"")
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(name, _)| name))
        .collect();
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words)?;
    let target = out.join("LICENSE.md");
    std::fs::write(&target, show("LICENSE.md")?).map_err(|source| Error::Write {
        path: target,
        source,
    })?;
    println!("wasi@preview1: {count} words");
    Ok(())
}

/// Writes `words.fst`, the WebAssembly instructions the specification's index lists whose
/// text-format name has no dot, the W3C notice its license requires on every copy, and the
/// statement naming the document they derive from.
fn wasm(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/wasm/SPEC").trim_ascii();
    let show = pinned_files(WASM_REPOSITORY, "wasm-spec", pinned)?;
    let macros = show("document/core/util/macros.def")?;
    let instructions: HashMap<&str, String> =
        macros.lines().filter_map(instruction_macro).collect();
    let index = show("document/core/appendix/index-instructions.py")?;
    let words: BTreeSet<&str> = index
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("Instruction("))
        .filter_map(|row| row.split_once(r"r'\").map(|(_, rest)| rest))
        .filter_map(|rest| {
            // A lone macro, its operands after `~`, is one instruction, not a `\I32.\ADD` pair.
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric())?;
            matches!(rest.as_bytes()[end], b'~' | b'\'').then(|| &rest[..end])
        })
        .filter_map(|name| instructions.get(name).map(String::as_str))
        .filter(|text| !text.contains('.'))
        .collect();
    let conf = show("document/core/conf.py")?;
    let copyright = conf
        .lines()
        .find_map(|line| line.strip_prefix("copyright = u'")?.strip_suffix('\''))
        .expect("the specification's conf.py states its copyright");
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words)?;
    let license = out.join("LICENSE");
    std::fs::write(&license, show("document/core/LICENSE")?).map_err(|source| Error::Write {
        path: license,
        source,
    })?;
    let notice = out.join("NOTICE");
    let statement = format!(
        "words.fst includes material derived from the WebAssembly Specification,\n{WASM_REPOSITORY}/tree/{pinned}/document/core.\nCopyright {copyright}.\n"
    );
    std::fs::write(&notice, statement).map_err(|source| Error::Write {
        path: notice,
        source,
    })?;
    println!("wasm@3.0: {count} words");
    Ok(())
}

/// The name and text-format spelling of an instruction that a `macros.def` line defines.
fn instruction_macro(line: &str) -> Option<(&str, String)> {
    let (name, body) = line.strip_prefix(".. |")?.split_once("| mathdef:: ")?;
    let text = body
        .strip_prefix(r"\xref{syntax/instructions}")?
        .split_once(r"\K{")?
        .1;
    let mut depth = 0_usize;
    let end = text.char_indices().find_map(|(at, c)| match c {
        '{' => {
            depth += 1;
            None
        }
        '}' if depth == 0 => Some(at),
        '}' => {
            depth -= 1;
            None
        }
        _ => None,
    })?;
    Some((name, text[..end].replace(r"\_", "_").replace("{.}", ".")))
}

/// Writes `words.fst`, the compound class names and camelCase method names of the public
/// Android framework API, and the framework's `NOTICE`, which the Apache License requires
/// with any copy.
fn android(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/android/AOSP").trim_ascii();
    let show = pinned_files(AOSP_FRAMEWORKS_BASE, "android", pinned)?;
    let api = show("core/api/current.txt")?;
    let mut words = BTreeSet::new();
    for line in api.lines() {
        let line = without_annotations(line);
        if let Some(declaration) = line.strip_prefix("  ").filter(|d| !d.starts_with(' ')) {
            let mut tokens = declaration.split_whitespace();
            if tokens.any(|token| matches!(token, "class" | "interface" | "enum"))
                && let Some(name) = tokens.next()
            {
                let name = name
                    .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                    .next()
                    .unwrap_or_default();
                let simple = name.rsplit('.').next().unwrap_or_default();
                // A single word such as `Builder` names too many Rust types to claim.
                let mut chars = simple.chars();
                if chars.next().is_some_and(|c| c.is_ascii_uppercase())
                    && chars.any(|c| c.is_ascii_uppercase())
                {
                    words.insert(simple.to_owned());
                }
            }
        } else if let Some((head, _)) = line
            .strip_prefix("    method ")
            .and_then(|declaration| declaration.split_once('('))
        {
            let name = head
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                .next()
                .unwrap_or_default();
            let lower = name
                .chars()
                .take_while(|c| !c.is_ascii_uppercase())
                .collect::<String>();
            if lower.len() < name.len()
                && lower.starts_with(|c: char| c.is_ascii_lowercase())
                && lower
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            {
                words.insert(name.to_owned());
            }
        }
    }
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let notice = out.join("NOTICE");
    std::fs::write(&notice, show("NOTICE")?).map_err(|source| Error::Write {
        path: notice,
        source,
    })?;
    println!("android@36: {count} words");
    Ok(())
}

/// `line` without its `@Name` and `@Name(arguments)` annotations, keeping `@interface`.
fn without_annotations(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find('@') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(after.len());
        if &after[..end] == "interface" {
            out.push_str("interface");
            rest = &after[end..];
            continue;
        }
        let mut tail = &after[end..];
        if tail.starts_with('(') {
            let (mut depth, mut quoted) = (0_usize, false);
            let close = tail.char_indices().find_map(|(at, c)| {
                match c {
                    '"' => quoted = !quoted,
                    '(' if !quoted => depth += 1,
                    ')' if !quoted => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(at);
                        }
                    }
                    _ => {}
                }
                None
            });
            tail = close.map_or("", |at| &tail[at + 1..]);
        }
        rest = tail.trim_start();
    }
    out.push_str(rest);
    out
}

/// Runs `program` in `dir` and returns its standard output.
fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String, Error> {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|source| Error::Spawn {
            program: program.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Failed {
            program: program.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn create_dir(dir: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dir).map_err(|source| Error::Write {
        path: dir.to_path_buf(),
        source,
    })
}

/// Writes the sorted, deduplicated `items` as an fst set to `file`, returning their count.
fn write_set<'a>(file: &Path, items: impl IntoIterator<Item = &'a str>) -> Result<usize, Error> {
    let set = fst::Set::from_iter(items)?;
    std::fs::write(file, set.as_fst().as_bytes()).map_err(|source| Error::Write {
        path: file.to_path_buf(),
        source,
    })?;
    Ok(set.len())
}

fn sysroot() -> Result<PathBuf, Error> {
    let output = std::process::Command::new("rustc")
        .arg(format!("+{TOOLCHAIN}"))
        .args(["--print", "sysroot"])
        .output()
        .map_err(Error::Rustc)?;
    if !output.status.success() {
        return Err(Error::Toolchain);
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn load(path: &Path) -> Result<Crate, Error> {
    let bytes = std::fs::read(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let krate: Crate = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    if krate.format_version != rustdoc_types::FORMAT_VERSION {
        return Err(Error::Format {
            path: path.to_path_buf(),
            found: krate.format_version,
            expected: rustdoc_types::FORMAT_VERSION,
        });
    }
    Ok(krate)
}

/// The paths found so far and the modules already walked under a prefix.
#[derive(Default)]
struct Paths {
    found: BTreeSet<Vec<String>>,
    walked: HashSet<(usize, Id, Vec<String>)>,
}

/// The standard library crates with cross-crate lookups.
struct Library<'a> {
    crates: &'a [Crate],
    /// Local items of each crate by path and kind.
    by_path: Vec<HashMap<(&'a [String], ItemKind), Id>>,
    /// Impl blocks of every crate, by the type they are for.
    impls: HashMap<(usize, Id), Vec<(usize, Id)>>,
    /// Impl blocks for primitive types, by primitive name.
    primitive_impls: HashMap<String, Vec<(usize, Id)>>,
}

impl<'a> Library<'a> {
    fn new(crates: &'a [Crate]) -> Self {
        let by_path = crates
            .iter()
            .map(|krate| {
                krate
                    .paths
                    .iter()
                    .filter(|(_, summary)| summary.crate_id == 0)
                    .map(|(id, summary)| ((summary.path.as_slice(), summary.kind), *id))
                    .collect()
            })
            .collect();
        let mut library = Self {
            crates,
            by_path,
            impls: HashMap::new(),
            primitive_impls: HashMap::new(),
        };
        for (index, krate) in crates.iter().enumerate() {
            for (id, item) in &krate.index {
                let ItemEnum::Impl(block) = &item.inner else {
                    continue;
                };
                if block.is_synthetic || block.is_negative || block.blanket_impl.is_some() {
                    continue;
                }
                match &block.for_ {
                    Type::ResolvedPath(path) => {
                        if let Some(target) = library.resolve(index, path.id) {
                            library.impls.entry(target).or_default().push((index, *id));
                        }
                    }
                    other => {
                        if let Some(name) = primitive_name(other) {
                            library
                                .primitive_impls
                                .entry(name)
                                .or_default()
                                .push((index, *id));
                        }
                    }
                }
            }
        }
        library
    }

    fn item(&self, krate: usize, id: Id) -> Option<&'a Item> {
        self.crates[krate].index.get(&id)
    }

    /// The crate and local id of `id` as referenced from `krate`.
    fn resolve(&self, krate: usize, id: Id) -> Option<(usize, Id)> {
        if self.crates[krate].index.contains_key(&id) {
            return Some((krate, id));
        }
        let summary = self.crates[krate].paths.get(&id)?;
        let external = self.crates[krate].external_crates.get(&summary.crate_id)?;
        let target = LAYERS.iter().position(|name| *name == external.name)?;
        let local = *self.by_path[target].get(&(summary.path.as_slice(), summary.kind))?;
        self.crates[target]
            .index
            .contains_key(&local)
            .then_some((target, local))
    }

    fn module(&self, krate: usize, id: Id, prefix: &mut Vec<String>, paths: &mut Paths) {
        const MAX_DEPTH: usize = 16;
        if prefix.len() > MAX_DEPTH || !paths.walked.insert((krate, id, prefix.clone())) {
            return;
        }
        let Some(Item {
            inner: ItemEnum::Module(module),
            ..
        }) = self.item(krate, id)
        else {
            return;
        };
        for child in &module.items {
            if let Some(item) = self.item(krate, *child) {
                self.add(krate, item, None, prefix, paths);
            }
        }
    }

    fn add(
        &self,
        krate: usize,
        item: &Item,
        rename: Option<&str>,
        prefix: &mut Vec<String>,
        paths: &mut Paths,
    ) {
        if let ItemEnum::Use(import) = &item.inner {
            let Some((target_crate, target)) = import.id.and_then(|id| self.resolve(krate, id))
            else {
                return;
            };
            let Some(target_item) = self.item(target_crate, target) else {
                return;
            };
            if !import.is_glob {
                self.add(target_crate, target_item, Some(&import.name), prefix, paths);
            } else if matches!(target_item.inner, ItemEnum::Module(_)) {
                self.module(target_crate, target, prefix, paths);
            } else if let ItemEnum::Enum(enumeration) = &target_item.inner {
                for variant in &enumeration.variants {
                    if let Some(name) = self
                        .item(target_crate, *variant)
                        .and_then(|v| v.name.as_ref())
                    {
                        let mut path = prefix.clone();
                        path.push(name.clone());
                        paths.found.insert(path);
                    }
                }
            }
            return;
        }
        let public = matches!(item.visibility, Visibility::Public);
        if rename.is_none() && !public && !matches!(item.inner, ItemEnum::Primitive(_)) {
            return;
        }
        let Some(name) = rename.or(item.name.as_deref()) else {
            return;
        };
        prefix.push(name.to_owned());
        paths.found.insert(prefix.clone());
        if matches!(item.inner, ItemEnum::Module(_)) {
            self.module(krate, item.id, prefix, paths);
        } else {
            for member in self.members(krate, item) {
                let mut path = prefix.clone();
                path.push(member);
                paths.found.insert(path);
            }
        }
        prefix.pop();
    }

    /// Fields, variants, trait items and the items of every non-blanket impl of `item`.
    fn members(&self, krate: usize, item: &Item) -> Vec<String> {
        let mut ids: Vec<(usize, Id)> = Vec::new();
        match &item.inner {
            ItemEnum::Struct(structure) => {
                if let StructKind::Plain { fields, .. } = &structure.kind {
                    ids.extend(fields.iter().map(|id| (krate, *id)));
                }
            }
            ItemEnum::Union(union) => ids.extend(union.fields.iter().map(|id| (krate, *id))),
            ItemEnum::Enum(enumeration) => {
                ids.extend(enumeration.variants.iter().map(|id| (krate, *id)));
            }
            ItemEnum::Trait(trait_) => ids.extend(trait_.items.iter().map(|id| (krate, *id))),
            _ => {}
        }
        let impls = match &item.inner {
            ItemEnum::Primitive(primitive) => self.primitive_impls.get(&primitive.name),
            _ => self.impls.get(&(krate, item.id)),
        };
        let mut names = Vec::new();
        for &(impl_crate, impl_id) in impls.into_iter().flatten() {
            let Some(ItemEnum::Impl(block)) = self.item(impl_crate, impl_id).map(|i| &i.inner)
            else {
                continue;
            };
            ids.extend(block.items.iter().map(|id| (impl_crate, *id)));
            names.extend(block.provided_trait_methods.iter().cloned());
        }
        for (member_crate, id) in ids {
            if let Some(member) = self.item(member_crate, id)
                && matches!(member.visibility, Visibility::Public | Visibility::Default)
                && let Some(name) = &member.name
            {
                names.push(name.clone());
            }
        }
        names
    }
}

/// The rustdoc primitive name of a non-path impl target.
fn primitive_name(ty: &Type) -> Option<String> {
    Some(match ty {
        Type::Primitive(name) => name.clone(),
        Type::Slice(_) => "slice".to_owned(),
        Type::Array { .. } => "array".to_owned(),
        Type::Tuple(_) => "tuple".to_owned(),
        Type::RawPointer { .. } => "pointer".to_owned(),
        Type::BorrowedRef { .. } => "reference".to_owned(),
        _ => return None,
    })
}
