//! Items a range changed without removing them, and the spellings that name exactly one of them.

use crate::definition::Definition;
use crate::index::Index;
use crate::intern::{Interner, PathId, Sym};
use crate::{ChangedPart, HashMap, HashSet, ItemKind};

/// The items head declares differently from the range's starting revision.
#[derive(Default)]
pub(crate) struct Changed {
    /// What changed in each changed item.
    pub(crate) parts: HashMap<PathId, ChangedPart>,
    /// The changed enums and structs the range only added variants or named fields to, each
    /// with the names of its members at head.
    grown: HashMap<PathId, HashSet<Sym>>,
    /// The changed item each path suffix of two or more segments names, `None` when the
    /// suffix also ends a path standing for another item.
    by_suffix: HashMap<Box<[Sym]>, Option<PathId>>,
    /// The items carrying the last segment of a changed item's path.
    by_name: HashMap<Sym, Vec<Carrier>>,
}

/// An item carrying a name.
struct Carrier {
    item: PathId,
    /// Whether some path standing for it is reachable from outside its crate.
    public: bool,
    /// The crate declaring it.
    krate: Sym,
}

impl Changed {
    /// The items whose definition in `head` differs from `start` in a part up to `compare`,
    /// told apart from the items of `head` and of the `dependencies` sharing a name.
    pub(crate) fn new(
        interner: &Interner,
        head: &Index,
        dependencies: &HashSet<PathId>,
        start: &HashMap<PathId, Definition>,
        compare: ChangedPart,
    ) -> Self {
        let parts: HashMap<PathId, ChangedPart> = head
            .definitions
            .iter()
            .filter_map(|(&id, &after)| Some((id, changed(*start.get(&id)?, after, compare)?)))
            .collect();
        let grown = grown(interner, &head.definitions, start, &parts);
        let mut by_suffix: HashMap<Box<[Sym]>, Option<PathId>> = HashMap::new();
        let mut by_name: HashMap<Sym, Vec<Carrier>> = HashMap::new();
        for &id in &head.paths {
            let item = origin(head, id);
            if !parts.contains_key(&item) {
                continue;
            }
            let segments = interner.segments(id);
            for suffix in suffixes(segments) {
                by_suffix
                    .entry(Box::from(suffix))
                    .and_modify(|named| {
                        if *named != Some(item) {
                            *named = None;
                        }
                    })
                    .or_insert(Some(item));
            }
            if let Some(&name) = segments.last() {
                by_name.entry(name).or_default();
            }
        }
        if !parts.is_empty() {
            let named = head
                .paths
                .iter()
                .map(|&id| (id, origin(head, id), head.public.contains(&id)));
            let dependencies = dependencies.iter().map(|&id| (id, id, true));
            for (id, item, public) in named.chain(dependencies) {
                let kind = interner.kind(id);
                if kind.is_manifest_entry() || kind == ItemKind::Serialized {
                    continue;
                }
                let segments = interner.segments(id);
                for suffix in suffixes(segments) {
                    if let Some(named) = by_suffix.get_mut(suffix)
                        && *named != Some(item)
                    {
                        *named = None;
                    }
                }
                let Some(carriers) = segments.last().and_then(|name| by_name.get_mut(name)) else {
                    continue;
                };
                match carriers.iter_mut().find(|carrier| carrier.item == item) {
                    Some(carrier) => carrier.public |= public,
                    None => carriers.push(Carrier {
                        item,
                        public,
                        krate: interner.segments(item)[0],
                    }),
                }
            }
        }
        Self {
            parts,
            grown,
            by_suffix,
            by_name,
        }
    }

    /// The changed item `suffix` names, when it ends paths standing for that item alone.
    pub(crate) fn named(&self, suffix: &[Sym]) -> Option<PathId> {
        self.by_suffix.get(suffix).copied().flatten()
    }

    /// The changed item the bare `name` names, read in crate `own` or outside every crate: the
    /// only item carrying it, else the only one that is public or of `own`.
    pub(crate) fn named_bare(&self, name: Sym, own: Option<Sym>) -> Option<PathId> {
        let carriers = self.by_name.get(&name)?;
        let item = if let [only] = carriers.as_slice() {
            only.item
        } else {
            let mut visible = carriers
                .iter()
                .filter(|carrier| carrier.public || Some(carrier.krate) == own);
            let (Some(carrier), None) = (visible.next(), visible.next()) else {
                return None;
            };
            carrier.item
        };
        self.parts.contains_key(&item).then_some(item)
    }

    /// The names of the members of `item` when the range only added variants or named fields
    /// to it, which text must name beside it to be able to describe what grew.
    pub(crate) fn grown(&self, item: PathId) -> Option<&HashSet<Sym>> {
        self.grown.get(&item)
    }

    /// Whether any spelling names a changed item.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The part of a definition that differs between `before` and `after`, the shape first,
/// looking no further than `compare`.
fn changed(before: Definition, after: Definition, compare: ChangedPart) -> Option<ChangedPart> {
    if before.shape != after.shape {
        Some(ChangedPart::Shape)
    } else if compare >= ChangedPart::Value && before.value != after.value {
        Some(ChangedPart::Value)
    } else if compare >= ChangedPart::Body && before.body != after.body {
        Some(ChangedPart::Body)
    } else {
        None
    }
}

/// The items of `parts` whose shape `after` differs from `before` only by members added
/// where no existing member moves for code to observe, each with the names of its members
/// after. Every member before keeps its definition and its order among the others.
fn grown(
    interner: &Interner,
    after: &HashMap<PathId, Definition>,
    before: &HashMap<PathId, Definition>,
    parts: &HashMap<PathId, ChangedPart>,
) -> HashMap<PathId, HashSet<Sym>> {
    let owners: HashSet<PathId> = parts
        .iter()
        .filter(|&(id, &part)| {
            let frame = after[id].frame;
            part == ChangedPart::Shape && frame != 0 && frame == before[id].frame
        })
        .map(|(&id, _)| id)
        .collect();
    if owners.is_empty() {
        return HashMap::new();
    }
    let members = |definitions: &HashMap<PathId, Definition>| {
        let mut by_owner: HashMap<PathId, Vec<PathId>> = HashMap::new();
        for &id in definitions.keys() {
            if matches!(interner.kind(id), ItemKind::Variant | ItemKind::Field)
                && let Some(owner) = interner.parent(id).filter(|owner| owners.contains(owner))
            {
                by_owner.entry(owner).or_default().push(id);
            }
        }
        by_owner
    };
    let (old, new) = (members(before), members(after));
    new.into_iter()
        .filter(|(owner, now)| {
            let was = old.get(owner).map_or(&[][..], Vec::as_slice);
            let fixed = before[owner].fixed_order || after[owner].fixed_order;
            let mut places = Vec::with_capacity(was.len());
            for member in was {
                let (then, now) = (before[member], after.get(member));
                let Some(now) = now.filter(|now| now.shape == then.shape) else {
                    return false;
                };
                if then.place == 0 || now.place == 0 || fixed && then.place != now.place {
                    return false;
                }
                places.push((then.place, now.place));
            }
            places.sort_unstable();
            was.len() < now.len() && places.is_sorted_by_key(|&(_, now)| now)
        })
        .map(|(owner, now)| {
            let names = now
                .iter()
                .filter_map(|&member| interner.segments(member).last());
            (owner, names.copied().collect())
        })
        .collect()
}

/// The item path `id` of `head` stands for: itself when declared, else the declared path its
/// re-export, alias copy or lent member stands for.
fn origin(head: &Index, id: PathId) -> PathId {
    if head.definitions.contains_key(&id) {
        return id;
    }
    head.origins.get(&id).copied().unwrap_or(id)
}

/// The suffixes of `segments` with at least two segments, longest first.
fn suffixes(segments: &[Sym]) -> impl Iterator<Item = &[Sym]> {
    std::iter::successors(Some(segments), |rest| rest.get(1..)).take_while(|rest| rest.len() >= 2)
}
