//! Interned identifiers and item paths shared by every snapshot of a scan.

use std::hash::BuildHasher;

use hashbrown::{DefaultHashBuilder, HashTable};

use crate::{HashMap, ItemKind};

/// An interned identifier.
pub(crate) type Sym = u32;

/// An interned item path.
pub(crate) type PathId = u32;

/// Identifiers and paths, each stored once.
#[derive(Default)]
pub(crate) struct Interner {
    syms: HashMap<Box<str>, Sym>,
    names: Vec<Box<str>>,
    /// The segments of every path, one path after another.
    arena: Vec<Sym>,
    /// Where the segments of each path end in `arena`.
    ends: Vec<u32>,
    /// Every path, found by the hash of its segments.
    paths: HashTable<PathId>,
    state: DefaultHashBuilder,
    kinds: Vec<ItemKind>,
    /// The path one segment shorter than each path, [`NO_PARENT`] while that is not interned.
    parents: Vec<PathId>,
    /// Paths interned before the path one segment shorter, by that path's segments.
    orphans: HashTable<(Box<[Sym]>, Vec<PathId>)>,
    /// Paths [`Self::child`] found, by their parent and last segment.
    children: HashMap<(PathId, Sym), PathId>,
    /// The segments [`Self::child`] builds a path in.
    scratch: Vec<Sym>,
}

/// The parent of a path whose prefix one segment shorter is not interned.
const NO_PARENT: PathId = PathId::MAX;

impl Interner {
    /// The symbol of `name`, interning it when new.
    pub(crate) fn sym(&mut self, name: &str) -> Sym {
        if let Some(&sym) = self.syms.get(name) {
            return sym;
        }
        let sym = Sym::try_from(self.names.len()).expect("fewer than 2^32 identifiers");
        self.names.push(name.into());
        self.syms.insert(name.into(), sym);
        sym
    }

    /// The symbol of `name` when it was ever interned.
    pub(crate) fn find_sym(&self, name: &str) -> Option<Sym> {
        self.syms.get(name).copied()
    }

    /// The identifier behind `sym`.
    pub(crate) fn name(&self, sym: Sym) -> &str {
        &self.names[index(sym)]
    }

    /// The id of `segments`, interning it with `kind` when new. The first kind seen sticks.
    pub(crate) fn path(&mut self, segments: &[Sym], kind: ItemKind) -> PathId {
        self.intern(segments, kind, None)
    }

    /// [`Self::path`], given the parent of `segments` when the caller knows it.
    fn intern(&mut self, segments: &[Sym], kind: ItemKind, parent: Option<PathId>) -> PathId {
        let hash = self.state.hash_one(segments);
        if let Some(id) = self.find_hashed(hash, segments) {
            return id;
        }
        let id = PathId::try_from(self.ends.len())
            .ok()
            .filter(|&id| id != NO_PARENT)
            .expect("fewer than 2^32 - 1 paths");
        let parent = parent.or_else(|| {
            let (_, above) = segments.split_last()?;
            if above.is_empty() {
                return None;
            }
            let above_hash = self.state.hash_one(above);
            let found = self.find_hashed(above_hash, above);
            if found.is_none() {
                let Self { orphans, state, .. } = self;
                orphans
                    .entry(
                        above_hash,
                        |(waiting_for, _)| **waiting_for == *above,
                        |(waiting_for, _)| state.hash_one(&**waiting_for),
                    )
                    .or_insert_with(|| (above.into(), Vec::new()))
                    .into_mut()
                    .1
                    .push(id);
            }
            found
        });
        self.arena.extend_from_slice(segments);
        let end = u32::try_from(self.arena.len()).expect("fewer than 2^32 segments");
        self.ends.push(end);
        let Self {
            arena,
            ends,
            paths,
            state,
            kinds,
            parents,
            orphans,
            ..
        } = self;
        paths.insert_unique(hash, id, |&known| state.hash_one(span(arena, ends, known)));
        kinds.push(kind);
        parents.push(parent.unwrap_or(NO_PARENT));
        if let Ok(waiting) = orphans.find_entry(hash, |(waiting_for, _)| **waiting_for == *segments)
        {
            let ((_, children), _) = waiting.remove();
            for child in children {
                parents[index(child)] = id;
            }
        }
        id
    }

    /// The id of `segments` when it was ever interned.
    pub(crate) fn find_path(&self, segments: &[Sym]) -> Option<PathId> {
        self.find_hashed(self.state.hash_one(segments), segments)
    }

    /// [`Self::find_path`] given the hash of `segments`.
    fn find_hashed(&self, hash: u64, segments: &[Sym]) -> Option<PathId> {
        self.paths
            .find(hash, |&known| {
                span(&self.arena, &self.ends, known) == segments
            })
            .copied()
    }

    /// The number of paths interned so far.
    pub(crate) fn path_count(&self) -> usize {
        self.ends.len()
    }

    /// The id of `parent`'s segments followed by `last`, interning it with `kind` when new as
    /// [`Self::path`] does, without hashing the whole segment list again for a pair seen before.
    pub(crate) fn child(&mut self, parent: PathId, last: Sym, kind: ItemKind) -> PathId {
        if let Some(&id) = self.children.get(&(parent, last)) {
            return id;
        }
        let mut path = std::mem::take(&mut self.scratch);
        path.clear();
        path.extend_from_slice(self.segments(parent));
        path.push(last);
        let id = self.intern(&path, kind, Some(parent));
        self.scratch = path;
        self.children.insert((parent, last), id);
        id
    }

    /// The path one segment shorter than `id`, when interned.
    pub(crate) fn parent(&self, id: PathId) -> Option<PathId> {
        Some(self.parents[index(id)]).filter(|&parent| parent != NO_PARENT)
    }

    /// The interned prefixes of `segments` with their lengths, longest first, `segments` itself
    /// included.
    pub(crate) fn prefixes<'a>(&'a self, segments: &'a [Sym]) -> Prefixes<'a> {
        Prefixes {
            interner: self,
            segments,
            len: segments.len(),
            next: None,
        }
    }

    /// The interned prefixes of `id` shorter than it with their lengths, longest first.
    pub(crate) fn ancestors(&self, id: PathId) -> Prefixes<'_> {
        let segments = self.segments(id);
        Prefixes {
            interner: self,
            segments,
            len: segments.len().saturating_sub(1),
            next: self.parent(id),
        }
    }

    /// The segments of `id`.
    pub(crate) fn segments(&self, id: PathId) -> &[Sym] {
        span(&self.arena, &self.ends, id)
    }

    /// The kind `id` was first seen with.
    pub(crate) fn kind(&self, id: PathId) -> ItemKind {
        self.kinds[index(id)]
    }

    /// `id` spelled with `::` separators.
    pub(crate) fn display(&self, id: PathId) -> String {
        self.join(self.segments(id))
    }

    /// `segments` spelled with `::` separators.
    pub(crate) fn join(&self, segments: &[Sym]) -> String {
        let mut out = String::new();
        for (i, &sym) in segments.iter().enumerate() {
            if i > 0 {
                out.push_str("::");
            }
            out.push_str(self.name(sym));
        }
        out
    }
}

/// The interned prefixes of a path with their lengths, longest first, each found through the
/// recorded parent of the one before and by its segments where that parent is not interned.
pub(crate) struct Prefixes<'a> {
    interner: &'a Interner,
    segments: &'a [Sym],
    /// The length of the next prefix to report.
    len: usize,
    /// The next prefix, when the recorded parents already found it.
    next: Option<PathId>,
}

impl Iterator for Prefixes<'_> {
    type Item = (usize, PathId);

    fn next(&mut self) -> Option<(usize, PathId)> {
        while self.len > 0 {
            let len = self.len;
            let found = self
                .next
                .or_else(|| self.interner.find_path(&self.segments[..len]));
            self.len -= 1;
            self.next = found.and_then(|id| self.interner.parent(id));
            if let Some(id) = found {
                return Some((len, id));
            }
        }
        None
    }
}

fn index(id: u32) -> usize {
    usize::try_from(id).expect("u32 ids fit in usize on supported targets")
}

/// The segments of path `id` in `arena`, whose paths end at `ends`.
fn span<'a>(arena: &'a [Sym], ends: &[u32], id: PathId) -> &'a [Sym] {
    let id = index(id);
    let start = id.checked_sub(1).map_or(0, |before| index(ends[before]));
    &arena[start..index(ends[id])]
}

#[cfg(test)]
mod tests {
    use super::Interner;
    use crate::ItemKind;

    #[test]
    fn a_path_interned_before_its_parent_is_adopted_when_the_parent_arrives() {
        let mut interner = Interner::default();
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| interner.sym(name));
        let abc = interner.path(&[a, b, c], ItemKind::Function);
        assert_eq!(interner.parent(abc), None);
        let ab = interner.path(&[a, b], ItemKind::Module);
        assert_eq!(interner.parent(abc), Some(ab));
        assert_eq!(interner.parent(ab), None);
        let root = interner.path(&[a], ItemKind::Module);
        assert_eq!(interner.parent(ab), Some(root));
        assert_eq!(interner.child(ab, c, ItemKind::Function), abc);
        let abd = interner.child(ab, d, ItemKind::Function);
        assert_eq!(interner.parent(abd), Some(ab));
    }

    #[test]
    fn ancestors_skip_a_prefix_never_interned_and_find_the_shorter_ones() {
        let mut interner = Interner::default();
        let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| interner.sym(name));
        let root = interner.path(&[a], ItemKind::Module);
        let abc = interner.path(&[a, b, c], ItemKind::Module);
        let abcd = interner.path(&[a, b, c, d], ItemKind::Function);
        let found: Vec<_> = interner.ancestors(abcd).collect();
        assert_eq!(found, [(3, abc), (1, root)]);
        let prefixes: Vec<_> = interner.prefixes(&[a, b, c, d]).collect();
        assert_eq!(prefixes, [(4, abcd), (3, abc), (1, root)]);
    }
}
