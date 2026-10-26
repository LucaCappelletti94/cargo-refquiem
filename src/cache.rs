//! Parsed files by blob id and parsed macro expansions by their inputs, persisted between runs.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gix::ObjectId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::HashMap;
use crate::parse::FileItems;

/// Bumped whenever [`FileItems`] or what the parser records changes.
const SCHEMA: u32 = 16;

/// A parse outcome: the declarations, or the parser's message.
pub(crate) type Parsed = Rc<Result<FileItems, String>>;

/// Parse outcomes keyed by blob id, and the items of macro expansions keyed by a hash of
/// everything the expanded text depends on.
pub(crate) struct FileCache {
    files: Store,
    expansions: Store,
}

impl FileCache {
    /// The cache stored in `dir`, empty when missing or unreadable.
    pub(crate) fn load(dir: &Path) -> Self {
        Self {
            files: Store::load(dir, "files"),
            expansions: Store::load(dir, "expansions"),
        }
    }

    /// The parse outcome of blob `id`, parsing `read()` when not cached or when its entry no
    /// longer decodes.
    pub(crate) fn get_or_parse<E>(
        &mut self,
        id: ObjectId,
        read: impl FnOnce() -> Result<Vec<u8>, E>,
    ) -> Result<Parsed, E> {
        if let Some(parsed) = self.files.get(id) {
            return Ok(Rc::new(parsed));
        }
        let bytes = read()?;
        let parsed = match std::str::from_utf8(&bytes) {
            Ok(source) => crate::parse::parse(source),
            Err(error) => Err(format!("not UTF-8: {error}")),
        };
        self.files.insert(id, &parsed);
        Ok(Rc::new(parsed))
    }

    /// What the expansion `key` produced, when stored.
    pub(crate) fn expansion(&self, key: ObjectId) -> Option<Expanded> {
        self.expansions.get(key)
    }

    /// Stores what the expansion `key` produced.
    pub(crate) fn store_expansion(&mut self, key: ObjectId, expanded: &Expanded) {
        self.expansions.insert(key, expanded);
    }

    /// Writes back whatever was added. Failing to write only loses the speed-up.
    pub(crate) fn save(&self) {
        self.files.save();
        self.expansions.save();
    }
}

/// The parsed items of an expansion, including an expansion that produced nothing.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Expanded(Option<FileItems>);

impl Expanded {
    pub(crate) fn items(self) -> Option<FileItems> {
        self.0
    }
}

impl From<Option<FileItems>> for Expanded {
    fn from(items: Option<FileItems>) -> Self {
        Self(items)
    }
}

/// JSON values keyed by object id in one file, each kept as its text and decoded when asked
/// for, as what is built from it is memoized and a value is rarely read twice.
struct Store {
    file: PathBuf,
    entries: HashMap<ObjectId, Box<RawValue>>,
    dirty: bool,
}

impl Store {
    /// The `kind` entries stored in `dir`, empty when missing or unreadable.
    fn load(dir: &Path, kind: &str) -> Self {
        let file = dir.join(format!(
            "{kind}-{}-{SCHEMA}.json",
            env!("CARGO_PKG_VERSION")
        ));
        let stored: std::collections::HashMap<String, Box<RawValue>> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let entries = stored
            .into_iter()
            .filter_map(|(hex, raw)| Some((ObjectId::from_hex(hex.as_bytes()).ok()?, raw)))
            .collect();
        Self {
            file,
            entries,
            dirty: false,
        }
    }

    /// The value stored for `id`, `None` when there is none or it no longer decodes.
    fn get<T: DeserializeOwned>(&self, id: ObjectId) -> Option<T> {
        serde_json::from_str(self.entries.get(&id)?.get()).ok()
    }

    fn insert<T: Serialize>(&mut self, id: ObjectId, value: &T) {
        if let Ok(raw) = serde_json::value::to_raw_value(value) {
            self.entries.insert(id, raw);
            self.dirty = true;
        }
    }

    /// Writes the entries back when any was added.
    fn save(&self) {
        if !self.dirty {
            return;
        }
        let stored: std::collections::HashMap<String, &RawValue> = self
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

#[cfg(test)]
mod tests {
    use super::FileCache;
    use crate::ItemKind;

    #[test]
    fn a_persisted_parse_does_not_require_the_blob_reader() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("lib.rs");
        let bytes = b"pub struct Remembered;";
        std::fs::write(&source, bytes).unwrap();
        let id =
            gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, bytes).unwrap();
        let stored = dir.path().join("cache");
        let mut cache = FileCache::load(&stored);
        cache.get_or_parse(id, || std::fs::read(&source)).unwrap();
        cache.save();
        drop(cache);
        std::fs::remove_file(&source).unwrap();

        let mut cache = FileCache::load(&stored);
        let parsed = cache.get_or_parse(id, || std::fs::read(&source)).unwrap();
        let items = parsed.as_ref().as_ref().unwrap();
        let [item] = items.items.as_slice() else {
            panic!("expected the cached struct");
        };
        assert_eq!(item.path, ["Remembered"]);
        assert_eq!(item.kind, ItemKind::Struct);
        assert!(item.public);
    }
}
