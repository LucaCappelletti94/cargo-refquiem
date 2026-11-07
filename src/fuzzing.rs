//! Entry points for the fuzz targets, each running one reader the way a scan does.

/// Reads and locates the mentions of `source`, Rust when `rust` holds and Markdown otherwise.
///
/// # Errors
///
/// Returns an error when the Markdown reader fails.
///
/// # Panics
///
/// Panics when a mention, marker or embedded piece of SQL has invalid source offsets.
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
    for sql in &read.fences {
        for (at, _) in sql.text.char_indices() {
            let file = sql.file_offset(at);
            assert!(
                file < source.len() && source.is_char_boundary(file),
                "{sql:?}"
            );
        }
    }
    Ok(read.mentions.len())
}

/// Whether Rust `source` parses into declarations, as indexing reads it.
#[must_use]
pub fn declarations(source: &str) -> bool {
    crate::parse::parse(source).is_ok()
}

/// How many names and mentions `source` holds in every dialect, as reading head's `.sql` files
/// does.
///
/// # Panics
///
/// Panics when a refusal is placed before the first line or column, or a comment or mention
/// has invalid source offsets.
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
        let (declared, comments) = crate::sql::read(source, dialect);
        for refusal in &declared.refused {
            assert!(refusal.line >= 1 && refusal.column >= 1, "{refusal:?}");
        }
        let ranges = comments.runs.iter().flat_map(|run| &run.lines);
        for &(start, end) in ranges.chain(&comments.documentation) {
            assert!(source.get(start..end).is_some(), "{start}..{end}");
        }
        let read = crate::text::sql_file(source, &comments, true);
        for mention in &read.mentions {
            assert!(
                source.get(mention.start..mention.end).is_some(),
                "{mention:?}"
            );
        }
        declared.names.len() + read.mentions.len()
    })
    .sum()
}
