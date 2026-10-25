//! The vocabularies refquiem can use, and which the selected configuration uses.

use std::path::PathBuf;

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
}

/// The bundled vocabularies, as their source names them.
const BUILTINS: &[BuiltinVocabulary] = &[BuiltinVocabulary {
    id: "english",
    kind: "natural-language",
    case: "unicode-lowercase",
    source: "https://github.com/en-wl/wordlist",
    source_version: include_str!("../data/english/ESDB").trim_ascii(),
}];

/// Returns metadata for the bundled vocabularies.
#[must_use]
pub fn available() -> &'static [BuiltinVocabulary] {
    BUILTINS
}

/// The vocabularies the selected configuration uses.
#[derive(Debug, serde::Serialize)]
pub struct EffectiveVocabularies {
    /// The selected config path, including the default when it is absent.
    pub config: PathBuf,
    /// Whether Rust standard-library resolution is active.
    pub rust_stdlib: bool,
    /// The builtin vocabularies the selection includes.
    pub builtins: &'static [BuiltinVocabulary],
}

/// Resolves vocabulary selection from `options.root` and `options.config` without scanning history or source declarations.
///
/// # Errors
///
/// Fails when the repository, workspace, or selected configuration cannot be read or resolved.
pub fn effective(options: &Options) -> Result<EffectiveVocabularies, Error> {
    let repo = scan::repository(options)?;
    let (_, workspace_dir) = scan::workspace(options, &repo)?;
    let (config, selection) = scan::selected_config(options, &workspace_dir)?;
    let builtins: &[BuiltinVocabulary] = if selection.uses_english() {
        BUILTINS
    } else {
        &[]
    };
    Ok(EffectiveVocabularies {
        config,
        rust_stdlib: true,
        builtins,
    })
}
