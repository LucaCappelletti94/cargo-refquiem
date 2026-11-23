//! Items a range changed without removing them, and the spellings that name exactly one of them.

use crate::definition::{Definition, Member, Reach};
use crate::index::Index;
use crate::intern::{Interner, PathId, Sym};
use crate::{ChangedPart, HashMap, HashSet, ItemKind};

/// The items head declares differently from the range's starting revision.
#[derive(Default)]
pub(crate) struct Changed {
    /// What changed in each changed item.
    pub(crate) parts: HashMap<PathId, ChangedPart>,
    /// The changed enums, structs and traits whose shape the range changed only in their
    /// members, which text sees as far as it sees the members.
    framed: HashMap<PathId, Framed>,
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

/// A changed enum, struct or trait whose shape without its members the range left alone.
struct Framed {
    /// The module declaring it, whose descendants a member's [`Reach`] counts from.
    declaring: Box<[Sym]>,
    /// Whether its members have an order code observes, which only an enum or struct has.
    ordered: bool,
    /// Whether a member added before an existing one moves it for code to observe, as
    /// [`Definition::fixed_order`] says.
    fixed: bool,
    /// Its members at either end of the range, those declared at its start first by place.
    members: Vec<Slot>,
}

/// A member of a [`Framed`] item as the range's start and head declare it, `None` where one
/// lacks it.
struct Slot {
    name: Sym,
    then: Option<Member>,
    now: Option<Member>,
}

/// What text in a module sees of a changed item's change.
pub(crate) enum Seen<'c> {
    /// Nothing, as every member the range added, changed, moved or removed is out of reach.
    Nothing,
    /// Members added beside the ones it reaches, which text describes only beside a member
    /// it reaches at head, those [`Members`] yields.
    Grown(Members<'c>),
    /// A change to the item it reaches.
    Changed,
}

/// The names of the members of a [`Framed`] item at head that a module reaches.
pub(crate) struct Members<'c> {
    framed: &'c Framed,
    slots: std::slice::Iter<'c, Slot>,
    module: Option<&'c [Sym]>,
}

impl Iterator for Members<'_> {
    type Item = Sym;

    fn next(&mut self) -> Option<Sym> {
        let (framed, module) = (self.framed, self.module);
        self.slots
            .find(|slot| slot.now.is_some() && framed.reaches(slot, module))
            .map(|slot| slot.name)
    }
}

impl Framed {
    /// Whether text in `module` reaches `slot` at either end of the range, every member
    /// counting for text outside every module.
    fn reaches(&self, slot: &Slot, module: Option<&[Sym]>) -> bool {
        let Some(module) = module else {
            return true;
        };
        [slot.then, slot.now].into_iter().flatten().any(|member| {
            let shared = match member.reach {
                Reach::Owner => return true,
                Reach::Up(levels) => self.declaring.len().saturating_sub(levels.into()).max(1),
                Reach::Down(depth) => self.declaring.len().min(usize::from(depth) + 1),
            };
            module.starts_with(&self.declaring[..shared])
        })
    }
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
        let framed = framed(interner, &head.definitions, start, &parts);
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
            framed,
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

    /// What text in `module`, `None` outside every module, sees of the change to `item`.
    pub(crate) fn seen<'c>(&'c self, item: PathId, module: Option<&'c [Sym]>) -> Seen<'c> {
        let Some(framed) = self.framed.get(&item) else {
            return Seen::Changed;
        };
        let mut added = false;
        let mut last = 0;
        for slot in &framed.members {
            if !framed.reaches(slot, module) {
                continue;
            }
            match (slot.then, slot.now) {
                (None, Some(_)) => added = true,
                (Some(then), Some(now))
                    if then.shape == now.shape
                        && (!framed.ordered
                            || then.place != 0
                                && now.place > last
                                && (!framed.fixed || then.place == now.place)) =>
                {
                    last = now.place;
                }
                _ => return Seen::Changed,
            }
        }
        if !added {
            return Seen::Nothing;
        }
        Seen::Grown(Members {
            framed,
            slots: framed.members.iter(),
            module,
        })
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

/// The items of `parts` whose shape `after` differs from `before` only in their members, each
/// with its members at either end.
fn framed(
    interner: &Interner,
    after: &HashMap<PathId, Definition>,
    before: &HashMap<PathId, Definition>,
    parts: &HashMap<PathId, ChangedPart>,
) -> HashMap<PathId, Framed> {
    let mut framed: HashMap<PathId, Framed> = parts
        .iter()
        .filter(|&(id, &part)| {
            let frame = after[id].frame;
            part == ChangedPart::Shape && frame != 0 && frame == before[id].frame
        })
        .map(|(&id, _)| {
            let segments = interner.segments(id);
            let framed = Framed {
                declaring: Box::from(&segments[..segments.len() - 1]),
                ordered: interner.kind(id) != ItemKind::Trait,
                fixed: before[&id].fixed_order || after[&id].fixed_order,
                members: Vec::new(),
            };
            (id, framed)
        })
        .collect();
    if framed.is_empty() {
        return framed;
    }
    let mut slots: HashMap<PathId, Slot> = HashMap::new();
    for (definitions, at_head) in [(before, false), (after, true)] {
        for (&id, definition) in definitions {
            let Some(owner) = interner
                .parent(id)
                .filter(|owner| framed.contains_key(owner))
            else {
                continue;
            };
            let Some(member) = member(interner.kind(owner), interner.kind(id), definition) else {
                continue;
            };
            let Some(&name) = interner.segments(id).last() else {
                continue;
            };
            let slot = slots.entry(id).or_insert(Slot {
                name,
                then: None,
                now: None,
            });
            if at_head {
                slot.now = Some(member);
            } else {
                slot.then = Some(member);
            }
        }
    }
    for (id, slot) in slots {
        if let Some(owner) = interner.parent(id).and_then(|owner| framed.get_mut(&owner)) {
            owner.members.push(slot);
        }
    }
    for owner in framed.values_mut() {
        owner
            .members
            .sort_unstable_by_key(|slot| slot.then.map_or(u32::MAX, |then| then.place));
    }
    framed
}

/// The item of kind `kind` saying `definition` as a member of an owner of kind `owner`, `None`
/// unless it is one whose addition alone leaves text naming the owner without its members true.
fn member(owner: ItemKind, kind: ItemKind, definition: &Definition) -> Option<Member> {
    match owner {
        ItemKind::Trait => matches!(
            kind,
            ItemKind::Method | ItemKind::AssociatedConst | ItemKind::AssociatedType
        )
        .then_some(Member {
            shape: definition.shape,
            place: 0,
            reach: Reach::Owner,
        }),
        _ => definition.member,
    }
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
