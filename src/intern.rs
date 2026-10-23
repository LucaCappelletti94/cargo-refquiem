//! Interned identifiers and item paths shared by every snapshot of a scan.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};

use hashbrown::HashTable;

use crate::ItemKind;

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
    state: RandomState,
    kinds: Vec<ItemKind>,
    /// Paths [`Self::child`] found, by their parent and last segment.
    children: HashMap<(PathId, Sym), PathId>,
    /// The segments [`Self::child`] builds a path in.
    scratch: Vec<Sym>,
}

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
        let hash = self.state.hash_one(segments);
        let known = self.paths.find(hash, |&known| {
            span(&self.arena, &self.ends, known) == segments
        });
        if let Some(&id) = known {
            return id;
        }
        let id = PathId::try_from(self.ends.len()).expect("fewer than 2^32 paths");
        self.arena.extend_from_slice(segments);
        let end = u32::try_from(self.arena.len()).expect("fewer than 2^32 segments");
        self.ends.push(end);
        let Self {
            arena,
            ends,
            paths,
            state,
            ..
        } = self;
        paths.insert_unique(hash, id, |&known| state.hash_one(span(arena, ends, known)));
        self.kinds.push(kind);
        id
    }

    /// The id of `segments` when it was ever interned.
    pub(crate) fn find_path(&self, segments: &[Sym]) -> Option<PathId> {
        let hash = self.state.hash_one(segments);
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
        let id = self.path(&path, kind);
        self.scratch = path;
        self.children.insert((parent, last), id);
        id
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

fn index(id: u32) -> usize {
    usize::try_from(id).expect("u32 ids fit in usize on supported targets")
}

/// The segments of path `id` in `arena`, whose paths end at `ends`.
fn span<'a>(arena: &'a [Sym], ends: &[u32], id: PathId) -> &'a [Sym] {
    let id = index(id);
    let start = id.checked_sub(1).map_or(0, |before| index(ends[before]));
    &arena[start..index(ends[id])]
}
