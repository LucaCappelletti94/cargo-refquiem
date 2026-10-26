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
    #[error("{path} has SHA-256 {found}, the pin is {expected}")]
    Checksum {
        path: PathBuf,
        found: String,
        expected: &'static str,
    },
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

/// Writes the standard library layers and the English, SQL, WASI, WebAssembly, Android,
/// `UIKit`, POSIX, browser and Python word lists under `data/`.
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
    android(&data.join("android"))?;
    uikit(&data.join("uikit"))?;
    posix(&data.join("posix"))?;
    browser_compat(&data.join("bcd"))?;
    python(&data.join("python"))
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

/// Where .NET for iOS keeps its bindings of the Apple SDKs, whose attributes carry the
/// Objective-C names.
const MACIOS_REPOSITORY: &str = "https://github.com/dotnet/macios";

/// Where kernel.org publishes the Linux man-pages project's releases of the POSIX manual pages.
const MAN_PAGES_POSIX: &str =
    "https://mirrors.edge.kernel.org/pub/linux/docs/man-pages/man-pages-posix";

/// The SHA-256 of the pinned `man-pages-posix` release tarball.
const MAN_PAGES_POSIX_SHA256: &str =
    "ce67bb25b5048b20dad772e405a83f4bc70faf051afa289361c81f9660318bc3";

/// Where npm serves MDN's `browser-compat-data` release tarballs.
const BCD_PACKAGE: &str =
    "https://registry.npmjs.org/@mdn/browser-compat-data/-/browser-compat-data";

/// The SHA-256 of the pinned `browser-compat-data` release tarball.
const BCD_SHA256: &str = "4e7a8747cac26a68087df5f38405f834a352245283af1128f4cf189989bd691e";

/// Where python.org publishes the Python source releases, documentation included.
const PYTHON_RELEASES: &str = "https://www.python.org/ftp/python";

/// The SHA-256 of the pinned Python source release tarball.
const PYTHON_SHA256: &str = "8b5536d280142ecf6cdfe15d12a21ba1c65e300bbf7b28a8d0cdb849575797de";

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
                if is_compound_type(simple) {
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
            if is_camel_case(name) {
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
            tail = closing_paren(tail).map_or("", |at| &tail[at + 1..]);
        }
        rest = tail.trim_start();
    }
    out.push_str(rest);
    out
}

/// The byte offset of the parenthesis closing the one `text` opens, skipping quoted text.
fn closing_paren(text: &str) -> Option<usize> {
    let (mut depth, mut quoted) = (0_usize, false);
    text.char_indices().find_map(|(at, c)| {
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
    })
}

/// Whether `name` is a type name of two or more words, such as `NativeActivity`.
///
/// A single word such as `Builder` names too many Rust types to claim.
fn is_compound_type(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.any(|c| c.is_ascii_uppercase())
}

/// Whether `name` is a camelCase member name of two or more words, such as `onCreate`.
fn is_camel_case(name: &str) -> bool {
    let lower = name.len()
        - name
            .trim_start_matches(|c: char| !c.is_ascii_uppercase())
            .len();
    lower < name.len()
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name[..lower]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Writes `words.fst`, the compound Objective-C class and protocol names and the camelCase
/// selector heads that the .NET binding of `UIKit` declares, and the binding's MIT `LICENSE`,
/// which must accompany any copy.
fn uikit(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/uikit/MACIOS").trim_ascii();
    let show = pinned_files(MACIOS_REPOSITORY, "macios", pinned)?;
    let source = without_comments(&show("src/uikit.cs")?);
    let mut words = BTreeSet::new();
    for attribute in ["Export", "Bind"] {
        for arguments in attribute_arguments(&source, attribute) {
            if let Some((selector, _)) = arguments
                .trim_start()
                .strip_prefix('"')
                .and_then(|quoted| quoted.split_once('"'))
            {
                let head = selector.split(':').next().unwrap_or_default();
                if is_camel_case(head) {
                    words.insert(head.to_owned());
                }
            }
        }
    }
    for (attributes, name) in interface_declarations(&source) {
        let has = |attribute| attribute_arguments(&attributes, attribute).next().is_some();
        // Categories extend another type, and static holders gather C# fields.
        if (has("BaseType") || has("Protocol")) && !has("Category") && !has("Static") {
            let objective_c = ["BaseType", "Protocol"]
                .into_iter()
                .flat_map(|attribute| attribute_arguments(&attributes, attribute))
                .find_map(|arguments| named_string(arguments, "Name"))
                .unwrap_or(&name);
            if is_compound_type(objective_c) {
                words.insert(objective_c.to_owned());
            }
        }
    }
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let license = out.join("LICENSE");
    std::fs::write(&license, show("LICENSE")?).map_err(|source| Error::Write {
        path: license,
        source,
    })?;
    println!("uikit@27.1: {count} words");
    Ok(())
}

/// C# `source` without its comments, with each raw `"""` string reduced to `""`.
fn without_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(['"', '/']) {
        out.push_str(&rest[..at]);
        let from = &rest[at..];
        let (kept, skipped) = if let Some(raw) = from.strip_prefix(r#"""""#) {
            (
                "\"\"",
                raw.find(r#"""""#).map_or(raw.len(), |end| end + 3) + 3,
            )
        } else if let Some(quoted) = from.strip_prefix('"') {
            let end = quoted.find('"').map_or(quoted.len(), |end| end + 1) + 1;
            (&from[..end], end)
        } else if from.starts_with("//") {
            ("", from.find('\n').unwrap_or(from.len()))
        } else if let Some(block) = from.strip_prefix("/*") {
            ("", block.find("*/").map_or(block.len(), |end| end + 2) + 2)
        } else {
            ("/", 1)
        };
        out.push_str(kept);
        rest = &from[skipped.min(from.len())..];
    }
    out.push_str(rest);
    out
}

/// The attribute text and C# name of every `interface` declaration in comment-free `source`.
///
/// Statements end at `;`, `{` and `}` outside attribute brackets. Bracket depth restarts at
/// each preprocessor line, so a malformed attribute in a disabled branch stays contained.
fn interface_declarations(source: &str) -> Vec<(String, String)> {
    let mut declarations = Vec::new();
    let mut statement = String::new();
    let mut depth = 0_usize;
    for line in source.lines() {
        if line.trim_start().starts_with('#') {
            depth = 0;
            continue;
        }
        let mut quoted = false;
        for c in line.chars() {
            match c {
                '"' => quoted = !quoted,
                '[' if !quoted => depth += 1,
                ']' if !quoted => depth = depth.saturating_sub(1),
                ';' | '{' | '}' if !quoted && depth == 0 => {
                    if c == '{'
                        && let Some(declaration) = interface_declaration(&statement)
                    {
                        declarations.push(declaration);
                    }
                    statement.clear();
                    continue;
                }
                _ => {}
            }
            statement.push(c);
        }
        statement.push('\n');
    }
    declarations
}

/// The attribute text and name of the `interface` that `statement` declares.
fn interface_declaration(statement: &str) -> Option<(String, String)> {
    statement.match_indices("interface").find_map(|(at, _)| {
        let before = statement[..at].chars().next_back();
        if before.is_some_and(|c| !(c.is_whitespace() || c == ']')) {
            return None;
        }
        let after = &statement[at + "interface".len()..];
        let name = after.trim_start();
        if name.len() == after.len() {
            return None;
        }
        let end = name
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(name.len());
        (end > 0).then(|| (statement[..at].to_owned(), name[..end].to_owned()))
    })
}

/// The argument text of each `attribute` that opens a C# attribute list or follows a comma
/// in one, empty for an attribute written without arguments.
fn attribute_arguments<'a>(text: &'a str, attribute: &'a str) -> impl Iterator<Item = &'a str> {
    text.match_indices(attribute).filter_map(move |(at, _)| {
        if !text[..at].trim_end().ends_with(['[', ',']) {
            return None;
        }
        let after = &text[at + attribute.len()..];
        if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let arguments = after.trim_start();
        if arguments.starts_with('(') {
            closing_paren(arguments).map(|end| &arguments[1..end])
        } else {
            Some("")
        }
    })
}

/// The string that `arguments` assigns to `key`, as in `Name = "UIGraphicsPDFRenderer"`.
fn named_string<'a>(arguments: &'a str, key: &str) -> Option<&'a str> {
    arguments.match_indices(key).find_map(|(at, _)| {
        let before = arguments[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return None;
        }
        let value = arguments[at + key.len()..]
            .trim_start()
            .strip_prefix('=')?
            .trim_start()
            .strip_prefix('"')?;
        value.split_once('"').map(|(value, _)| value)
    })
}

/// Writes `words.fst`, the POSIX.1-2017 names that the `man-pages-posix` header and function
/// pages define, and `POSIX-COPYRIGHT`, the notice IEEE and The Open Group require on any
/// product containing the material.
///
/// The names are every uppercase header symbol with an inner underscore such as `O_RDONLY`,
/// the error numbers of `errno.h` such as `EINVAL`, the signals of `signal.h` such as
/// `SIGTERM`, and every function with an inner underscore such as `pthread_create`. A single
/// word such as `close` or `BUFSIZ` names too many Rust items to claim.
fn posix(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/posix/MAN_PAGES_POSIX").trim_ascii();
    let pages = pinned_tarball(
        &format!("{MAN_PAGES_POSIX}/{pinned}.tar.xz"),
        MAN_PAGES_POSIX_SHA256,
        pinned,
    )?;
    let mut words = BTreeSet::new();
    for (page, text) in files(&pages.join("man0p"))? {
        let text = without_font_escapes(&text);
        for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            let symbol = token.starts_with(|c: char| c.is_ascii_uppercase() || c == '_')
                && token.contains(|c: char| c.is_ascii_uppercase())
                && token
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if !symbol {
                continue;
            }
            // `LC_` stands for a family of names, not a name.
            let underscored = token.trim_matches('_').contains('_') && !token.ends_with('_');
            let error = page == "errno.h.0p" && token.len() > 2 && token.starts_with('E');
            let signal = page == "signal.h.0p" && token.len() > 3 && token.starts_with("SIG");
            if underscored || (!token.contains('_') && (error || signal)) {
                words.insert(token.to_owned());
            }
        }
    }
    for (_, text) in files(&pages.join("man3p"))? {
        let Some((_, after)) = text.split_once("\n.SH NAME\n") else {
            continue;
        };
        let section = after.split("\n.").next().unwrap_or_default();
        let names = section
            .split("\\(em")
            .next()
            .and_then(|names| names.split("\\-").next())
            .unwrap_or_default()
            .replace("\\%", "")
            .replace('\\', "");
        for name in names.split(|c: char| c == ',' || c.is_whitespace()) {
            let identifier = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if identifier && name.trim_matches('_').contains('_') {
                words.insert(name.to_owned());
            }
        }
    }
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let notice = out.join("POSIX-COPYRIGHT");
    std::fs::copy(pages.join("POSIX-COPYRIGHT"), &notice).map_err(|source| Error::Write {
        path: notice,
        source,
    })?;
    println!("posix@2017: {count} words");
    Ok(())
}

/// The file name and text of every file directly in `dir`, in file-name order.
fn files(dir: &Path) -> Result<Vec<(String, String)>, Error> {
    let read = |source| Error::Read {
        path: dir.to_owned(),
        source,
    };
    let mut pages = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(read)? {
        let path = entry.map_err(read)?.path();
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        pages.push((
            name.into_owned(),
            String::from_utf8_lossy(&bytes).into_owned(),
        ));
    }
    pages.sort_unstable();
    Ok(pages)
}

/// troff `text` without its comment lines and its `\fB`, `\fI`, `\fR`, `\fP` and `\f(XX`
/// font changes, so a name set in bold reads as one token.
fn without_font_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text
        .lines()
        .filter(|line| !line.starts_with(".\\\"") && !line.starts_with("'\\\""))
    {
        let mut rest = line;
        while let Some(at) = rest.find("\\f") {
            out.push_str(&rest[..at]);
            let after = &rest[at + 2..];
            rest = match after.strip_prefix('(') {
                Some(named) => named.get(2..).unwrap_or_default(),
                None => after.get(1..).unwrap_or_default(),
            };
        }
        out.push_str(rest);
        out.push('\n');
    }
    out
}

/// The directory named `name` where the tarball at `url` is unpacked, after its SHA-256
/// matched `sha256`.
fn pinned_tarball(url: &str, sha256: &'static str, name: &str) -> Result<PathBuf, Error> {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits inside the workspace")
        .join("target")
        .join("xtask");
    create_dir(&cache)?;
    let archive = cache.join(url.rsplit('/').next().unwrap_or(name));
    let archive_arg = archive.to_string_lossy();
    if !archive.is_file() {
        run(&cache, "curl", &["-sSfL", "-o", &archive_arg, url])?;
    }
    let found = run(&cache, "sha256sum", &[&archive_arg])?;
    let found = found.split_whitespace().next().unwrap_or_default();
    if found != sha256 {
        return Err(Error::Checksum {
            path: archive,
            found: found.to_owned(),
            expected: sha256,
        });
    }
    let dir = cache.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|source| Error::Write {
            path: dir.clone(),
            source,
        })?;
    }
    create_dir(&dir)?;
    run(&dir, "tar", &["-xf", &archive_arg, "--strip-components=1"])?;
    Ok(dir)
}

/// Writes the word lists drawn from MDN's `browser-compat-data`, `web-api.fst`,
/// `javascript.fst` and `svg.fst`, and the data's CC0 `LICENSE`.
fn browser_compat(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/bcd/BCD").trim_ascii();
    let version = pinned.trim_start_matches('v');
    let package = pinned_tarball(
        &format!("{BCD_PACKAGE}-{version}.tgz"),
        BCD_SHA256,
        &format!("browser-compat-data-{version}"),
    )?;
    let path = package.join("data.json");
    let bytes = std::fs::read(&path).map_err(|source| Error::Read {
        path: path.clone(),
        source,
    })?;
    let data: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| Error::Json { path, source })?;
    create_dir(out)?;
    for (file, id, words) in [
        ("web-api.fst", "web-api@2026-10", web_api_names(&data)),
        (
            "javascript.fst",
            "javascript@2026-10",
            javascript_names(&data),
        ),
        ("svg.fst", "svg@2026-10", svg_names(&data)),
    ] {
        let count = write_set(&out.join(file), words.iter().copied())?;
        println!("{id}: {count} words");
    }
    let license = out.join("LICENSE");
    std::fs::copy(package.join("LICENSE"), &license).map_err(|source| Error::Write {
        path: license,
        source,
    })?;
    Ok(())
}

/// Whether BCD marks `entry` as on the standards track, which it does unless it says otherwise.
fn standards_track(entry: &serde_json::Value) -> bool {
    entry["__compat"]["status"]["standard_track"]
        .as_bool()
        .unwrap_or(true)
}

/// Whether `name` is ASCII alphanumeric and camelCase such as `viewBox`, or compound with a
/// lowercase letter such as `ArrayBuffer`, leaving out constants such as `PI` and `AF_UNIX`.
fn is_mixed_case_name(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_alphanumeric())
        && (is_camel_case(name)
            || (is_compound_type(name) && name.contains(|c: char| c.is_ascii_lowercase())))
}

/// The standards-track Web API interface and member names that are compound type names such
/// as `HTMLElement` or camelCase such as `querySelector`.
///
/// Event, inheritance and permission entries describe behavior rather than name a member,
/// and a `_static` suffix marks a static member. A single word such as `fetch` or `Document`
/// names too many Rust items to claim.
fn web_api_names(data: &serde_json::Value) -> BTreeSet<&str> {
    let claimable = |name: &str| {
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && (is_compound_type(name) || is_camel_case(name))
    };
    let mut words = BTreeSet::new();
    for (interface, entry) in data["api"].as_object().into_iter().flatten() {
        if interface.starts_with("__") || !standards_track(entry) {
            continue;
        }
        if claimable(interface) {
            words.insert(interface.as_str());
        }
        for (member, entry) in entry.as_object().into_iter().flatten() {
            let described = ["_event", "_inheritance", "_permission"]
                .iter()
                .any(|suffix| member.ends_with(suffix));
            if member.starts_with("__")
                || member.starts_with("@@")
                || described
                || !standards_track(entry)
            {
                continue;
            }
            let member = member.strip_suffix("_static").unwrap_or(member);
            if claimable(member) {
                words.insert(member);
            }
        }
    }
    words
}

/// The standards-track JavaScript built-in globals and members that are mixed case, such as
/// `ArrayBuffer` and `hasOwnProperty`.
fn javascript_names(data: &serde_json::Value) -> BTreeSet<&str> {
    let mut words = BTreeSet::new();
    for (global, entry) in data["javascript"]["builtins"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if global.starts_with("__") || !standards_track(entry) {
            continue;
        }
        if is_mixed_case_name(global) {
            words.insert(global.as_str());
        }
        for (member, entry) in entry.as_object().into_iter().flatten() {
            if member.starts_with("__") || member.starts_with("@@") || !standards_track(entry) {
                continue;
            }
            let member = member.strip_suffix("_static").unwrap_or(member);
            if is_mixed_case_name(member) {
                words.insert(member);
            }
        }
    }
    words
}

/// The standards-track SVG element and attribute names that are camelCase, such as
/// `linearGradient` and `viewBox`, at any depth of BCD's `svg` tree.
fn svg_names(data: &serde_json::Value) -> BTreeSet<&str> {
    let mut words = BTreeSet::new();
    let mut pending = vec![&data["svg"]];
    while let Some(node) = pending.pop() {
        for (name, entry) in node.as_object().into_iter().flatten() {
            if name.starts_with("__") || !entry.is_object() || !standards_track(entry) {
                continue;
            }
            if is_mixed_case_name(name) {
                words.insert(name.as_str());
            }
            pending.push(entry);
        }
    }
    words
}

/// The Sphinx directives that document a Python object, whose argument starts with its name.
const PYTHON_OBJECT_DIRECTIVES: [&str; 15] = [
    "function",
    "class",
    "exception",
    "method",
    "attribute",
    "data",
    "classmethod",
    "staticmethod",
    "decorator",
    "decoratormethod",
    "property",
    "coroutinefunction",
    "coroutinemethod",
    "abstractmethod",
    "type",
];

/// Writes `words.fst`, the Python built-in and standard-library object names that the
/// Python documentation describes and that are compound such as `ValueError` or camelCase
/// such as `addHandler`, and the release's `LICENSE`, whose PSF notice must accompany any copy.
///
/// The last segment of a dotted name is the name a mention spells. Snake names such as
/// `read_text`, uppercase constants such as `AF_UNIX` and single words such as `len` name
/// too many Rust items to claim.
fn python(out: &Path) -> Result<(), Error> {
    let pinned = include_str!("../../data/python/CPYTHON").trim_ascii();
    let version = pinned.trim_start_matches('v');
    let source = pinned_tarball(
        &format!("{PYTHON_RELEASES}/{version}/Python-{version}.tar.xz"),
        PYTHON_SHA256,
        &format!("Python-{version}"),
    )?;
    let mut words = BTreeSet::new();
    for dir in ["builtins", "library"] {
        for (page, text) in files(&source.join("Doc").join(dir))? {
            let rst = Path::new(&page)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rst"));
            if !rst {
                continue;
            }
            for line in text.lines() {
                let Some((directive, argument)) = line
                    .trim_start()
                    .strip_prefix("..")
                    .and_then(|rest| rest.split_once("::"))
                else {
                    continue;
                };
                let directive = directive.trim();
                let directive = directive.strip_prefix("py:").unwrap_or(directive);
                if !PYTHON_OBJECT_DIRECTIVES.contains(&directive) {
                    continue;
                }
                let target = argument
                    .trim_start()
                    .split(|c: char| c.is_whitespace() || c == '(' || c == '[')
                    .next()
                    .unwrap_or_default();
                let name = target.rsplit('.').next().unwrap_or_default();
                if is_mixed_case_name(name) {
                    words.insert(name.to_owned());
                }
            }
        }
    }
    create_dir(out)?;
    let count = write_set(&out.join("words.fst"), words.iter().map(String::as_str))?;
    let license = out.join("LICENSE");
    std::fs::copy(source.join("LICENSE"), &license).map_err(|source| Error::Write {
        path: license,
        source,
    })?;
    println!("python@3.15: {count} words");
    Ok(())
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
