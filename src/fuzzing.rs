//! Entry points for the fuzz targets, each running one reader the way a scan does.

/// Reads and locates the mentions of `source`, Rust when `rust` holds and Markdown otherwise.
///
/// # Errors
///
/// Returns an error when the Markdown reader fails.
///
/// # Panics
///
/// Panics when a mention's offsets do not delimit text of `source`.
pub fn mentions(source: &str, rust: bool) -> Result<usize, crate::Error> {
    let spelled = if rust {
        crate::text::rust(source)
    } else {
        crate::text::markdown_file(source)
    }
    .map_err(|_| crate::Error::Markdown {
        path: "<fuzz input>".to_owned(),
    })?;
    let starts = crate::text::line_starts(source);
    for mention in &spelled {
        assert!(mention.start <= mention.end, "{mention:?}");
        let _ = &source[mention.start..mention.end];
        let _ = crate::text::line_column(&starts, source, mention.start);
    }
    Ok(spelled.len())
}

/// Whether Rust `source` parses into declarations, as indexing reads it.
#[must_use]
pub fn declarations(source: &str) -> bool {
    crate::parse::parse(source).is_ok()
}
