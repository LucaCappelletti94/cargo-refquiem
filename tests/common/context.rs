//! The mention context tests build from a source and a kind.

use refquiem::{Context, Kind, Source};

/// The mention context of `source` and `kind`.
pub fn context(source: Source, kind: Kind) -> Context {
    Context { source, kind }
}
