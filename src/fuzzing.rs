//! Entry points for the fuzz targets, each running one reader the way a scan does.

/// Reads and locates the mentions of `source`, Rust when `rust` holds and Markdown otherwise.
///
/// # Panics
///
/// When a mention's offsets do not delimit text of `source`, as a scan would.
#[must_use]
pub fn mentions(source: &str, rust: bool) -> usize {
    let spelled = if rust {
        crate::text::rust(source)
    } else {
        crate::text::markdown_file(source)
    };
    let starts = crate::text::line_starts(source);
    for mention in &spelled {
        assert!(mention.start <= mention.end, "{mention:?}");
        let _ = &source[mention.start..mention.end];
        let _ = crate::text::line_column(&starts, source, mention.start);
    }
    spelled.len()
}

/// Whether Rust `source` parses into declarations, as indexing reads it.
#[must_use]
pub fn declarations(source: &str) -> bool {
    crate::parse::parse(source).is_ok()
}
