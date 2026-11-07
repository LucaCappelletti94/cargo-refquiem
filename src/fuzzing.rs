//! Entry points for the fuzz targets, each running one reader the way a scan does.

/// Reads and locates the mentions of `source`, Rust when `rust` holds and Markdown otherwise.
///
/// # Errors
///
/// Returns an error when the Markdown reader fails.
///
/// # Panics
///
/// Panics when a mention or marker has invalid source offsets.
pub fn mentions(source: &str, rust: bool) -> Result<usize, crate::Error> {
    let read = if rust {
        crate::text::rust(source, false)
    } else {
        crate::text::markdown_file(source)
    }
    .map_err(|_| crate::Error::Markdown {
        path: "<fuzz input>".to_owned(),
    })?;
    let starts = crate::text::line_starts(source);
    for marker in &read.markers {
        let end = marker.start + marker.name.len();
        assert_eq!(&source[marker.start..end], marker.name);
        let _ = crate::text::line_column(&starts, source, marker.start);
    }
    for mention in &read.mentions {
        assert!(mention.start <= mention.end, "{mention:?}");
        let _ = &source[mention.start..mention.end];
        let _ = crate::text::line_column(&starts, source, mention.start);
    }
    Ok(read.mentions.len())
}

/// Whether Rust `source` parses into declarations, as indexing reads it.
#[must_use]
pub fn declarations(source: &str) -> bool {
    crate::parse::parse(source).is_ok()
}

/// How many names `source` declares in every dialect, as reading head's `.sql` files does.
///
/// # Panics
///
/// Panics when a refusal is placed before the first line or column.
#[must_use]
pub fn sql_declarations(source: &str) -> usize {
    use crate::sql::Dialect;
    [
        Dialect::Generic,
        Dialect::Postgres,
        Dialect::Mysql,
        Dialect::Sqlite,
    ]
    .into_iter()
    .map(|dialect| {
        let declared = crate::sql::declared(source, dialect);
        for refusal in &declared.refused {
            assert!(refusal.line >= 1 && refusal.column >= 1, "{refusal:?}");
        }
        declared.names.len()
    })
    .sum()
}
