//! The vocabularies refquiem can use, and which the selected configuration uses.

use std::path::PathBuf;

use crate::config;
use crate::scan;
use crate::{Error, Options};

/// Metadata for a bundled vocabulary.
#[derive(Debug, serde::Serialize)]
pub struct BuiltinVocabulary {
    /// The vocabulary id.
    pub id: &'static str,
    /// The natural-language or technical vocabulary category.
    pub kind: &'static str,
    /// The name-matching case rule.
    pub case: &'static str,
    /// The source the words come from.
    pub source: &'static str,
    /// The version of the source.
    pub source_version: &'static str,
    #[serde(skip)]
    builtin: config::Builtin,
}

/// The bundled vocabularies, as their source names them.
const BUILTINS: &[BuiltinVocabulary] = &[
    BuiltinVocabulary {
        id: "english",
        kind: "natural-language",
        case: "unicode-lowercase",
        source: "https://github.com/en-wl/wordlist",
        source_version: include_str!("../data/english/ESDB").trim_ascii(),
        builtin: config::Builtin::English,
    },
    BuiltinVocabulary {
        id: "sql@2023",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/postgres/postgres",
        source_version: include_str!("../data/sql/POSTGRESQL").trim_ascii(),
        builtin: config::Builtin::Sql2023,
    },
    BuiltinVocabulary {
        id: "wasi@preview1",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/WebAssembly/WASI",
        source_version: include_str!("../data/wasi/WASI").trim_ascii(),
        builtin: config::Builtin::WasiPreview1,
    },
    BuiltinVocabulary {
        id: "wasm@3.0",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/WebAssembly/spec",
        source_version: include_str!("../data/wasm/SPEC").trim_ascii(),
        builtin: config::Builtin::Wasm3,
    },
    BuiltinVocabulary {
        id: "android@36",
        kind: "technical",
        case: "sensitive",
        source: "https://android.googlesource.com/platform/frameworks/base",
        source_version: include_str!("../data/android/AOSP").trim_ascii(),
        builtin: config::Builtin::Android36,
    },
    BuiltinVocabulary {
        id: "uikit@27.1",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/dotnet/macios",
        source_version: include_str!("../data/uikit/MACIOS").trim_ascii(),
        builtin: config::Builtin::Uikit27_1,
    },
    BuiltinVocabulary {
        id: "posix@2017",
        kind: "technical",
        case: "sensitive",
        source: "https://mirrors.edge.kernel.org/pub/linux/docs/man-pages/man-pages-posix",
        source_version: include_str!("../data/posix/MAN_PAGES_POSIX").trim_ascii(),
        builtin: config::Builtin::Posix2017,
    },
    BuiltinVocabulary {
        id: "web-api@2026-10",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/mdn/browser-compat-data",
        source_version: include_str!("../data/bcd/BCD").trim_ascii(),
        builtin: config::Builtin::WebApi2026_10,
    },
    BuiltinVocabulary {
        id: "python@3.15",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/python/cpython",
        source_version: include_str!("../data/python/CPYTHON").trim_ascii(),
        builtin: config::Builtin::Python3_15,
    },
    BuiltinVocabulary {
        id: "javascript@2026-10",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/mdn/browser-compat-data",
        source_version: include_str!("../data/bcd/BCD").trim_ascii(),
        builtin: config::Builtin::Javascript2026_10,
    },
    BuiltinVocabulary {
        id: "svg@2026-10",
        kind: "technical",
        case: "sensitive",
        source: "https://github.com/mdn/browser-compat-data",
        source_version: include_str!("../data/bcd/BCD").trim_ascii(),
        builtin: config::Builtin::Svg2026_10,
    },
];

/// Returns metadata for the bundled vocabularies.
#[must_use]
pub fn available() -> &'static [BuiltinVocabulary] {
    BUILTINS
}

/// The id of `builtin` in the catalog.
pub(crate) fn id(builtin: config::Builtin) -> &'static str {
    BUILTINS
        .iter()
        .find(|vocabulary| vocabulary.builtin == builtin)
        .expect("the catalog lists every built-in")
        .id
}

/// Metadata for a selected custom vocabulary file.
#[derive(Debug, serde::Serialize)]
pub struct CustomVocabulary {
    /// The canonical path of the file.
    pub path: PathBuf,
    /// The natural-language or technical vocabulary category.
    pub kind: &'static str,
    /// The name-matching case rule.
    pub case: &'static str,
    /// The format version the file declares.
    pub format_version: u32,
}

/// The vocabularies the selected configuration uses.
#[derive(Debug, serde::Serialize)]
pub struct EffectiveVocabularies {
    /// The selected config path, including the default when it is absent.
    pub config: PathBuf,
    /// Whether Rust standard-library resolution is active.
    pub rust_stdlib: bool,
    /// The builtin vocabularies the selection includes, in catalog order.
    pub builtins: Vec<&'static BuiltinVocabulary>,
    /// The custom files the selection includes, in canonical path order.
    pub files: Vec<CustomVocabulary>,
}

/// Resolves vocabulary selection from `options.root` and `options.config` without scanning history or source declarations.
///
/// # Errors
///
/// Fails when the repository, workspace, or selected configuration cannot be read or resolved,
/// or a selected custom vocabulary file cannot be read or validated.
pub fn effective(options: &Options) -> Result<EffectiveVocabularies, Error> {
    let repo = scan::repository(options)?;
    let (_, workspace_dir) = scan::workspace(options, &repo)?;
    let (config_path, selection) = scan::selected_config(options, &workspace_dir)?;
    let builtins = BUILTINS
        .iter()
        .filter(|builtin| selection.selects(builtin.builtin))
        .collect();
    let files = config::load_custom(&config_path, &selection)?
        .into_iter()
        .map(|file| CustomVocabulary {
            path: file.path,
            kind: file.kind.as_str(),
            case: file.case.as_str(),
            format_version: file.format_version,
        })
        .collect();
    Ok(EffectiveVocabularies {
        config: config_path,
        rust_stdlib: true,
        builtins,
        files,
    })
}
