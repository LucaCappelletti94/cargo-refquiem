//! Parsed files by blob id, persisted between runs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gix::ObjectId;
use serde_json::value::RawValue;

use crate::parse::FileItems;

/// Bumped whenever [`FileItems`] or what the parser records changes.
const SCHEMA: u32 = 12;

/// A parse outcome: the declarations, or the parser's message.
pub(crate) type Parsed = Rc<Result<FileItems, String>>;

/// Parse outcomes keyed by blob id, each kept as its JSON text and decoded when asked for, as
/// the contribution built from it is memoized and the outcome itself is rarely read twice.
pub(crate) struct FileCache {
    file: PathBuf,
    entries: HashMap<ObjectId, Box<RawValue>>,
    dirty: bool,
}

impl FileCache {
    /// The cache stored in `dir`, empty when missing or unreadable.
    pub(crate) fn load(dir: &std::path::Path) -> Self {
        let file = dir.join(format!("files-{}-{SCHEMA}.json", env!("CARGO_PKG_VERSION")));
        let stored: HashMap<String, Box<RawValue>> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let entries = stored
            .into_iter()
            .filter_map(|(hex, parsed)| Some((ObjectId::from_hex(hex.as_bytes()).ok()?, parsed)))
            .collect();
        Self {
            file,
            entries,
            dirty: false,
        }
    }

    /// The parse outcome of blob `id`, parsing `read()` when not cached or when its entry no
    /// longer decodes.
    pub(crate) fn get_or_parse<E>(
        &mut self,
        id: ObjectId,
        read: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Parsed, E> {
        let stored = self
            .entries
            .get(&id)
            .and_then(|raw| serde_json::from_str(raw.get()).ok());
        if let Some(parsed) = stored {
            return Ok(Rc::new(parsed));
        }
        let bytes = read()?;
        let parsed = match std::str::from_utf8(&bytes) {
            Ok(source) => crate::parse::parse(source),
            Err(error) => Err(format!("not UTF-8: {error}")),
        };
        if let Ok(raw) = serde_json::value::to_raw_value(&parsed) {
            self.entries.insert(id, raw);
            self.dirty = true;
        }
        Ok(Rc::new(parsed))
    }

    /// Writes the cache back when anything was added. Failing to write only loses the speed-up.
    pub(crate) fn save(&self) {
        if !self.dirty {
            return;
        }
        let stored: HashMap<String, &RawValue> = self
            .entries
            .iter()
            .map(|(id, raw)| (id.to_string(), &**raw))
            .collect();
        let Ok(bytes) = serde_json::to_vec(&stored) else {
            return;
        };
        let Some(dir) = self.file.parent() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let temporary = self.file.with_extension("json.tmp");
        if std::fs::write(&temporary, bytes).is_ok() {
            let _ = std::fs::rename(&temporary, &self.file);
        }
    }
}
