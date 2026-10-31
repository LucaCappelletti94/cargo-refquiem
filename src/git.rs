//! Commit ranges and the files of a commit or of the working tree.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gix::ObjectId;

use crate::{Error, HashMap, HashSet, Range};

/// Read access to the files of one revision, by `/`-separated path from the repository root.
pub(crate) trait Snapshot {
    /// The blob id of the file at `path`, `None` when absent or not a file.
    fn blob(&self, path: &str) -> Result<Option<ObjectId>, Error>;
    /// The contents of the file at `path`, whose blob id is `id`.
    fn read(&self, path: &str, id: ObjectId) -> Result<Vec<u8>, Error>;
    /// The entries of directory `dir` as `(name, is_dir)`, empty when absent.
    fn list_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, Error>;
    /// Every file path.
    fn files(&self) -> Result<Vec<String>, Error>;
    /// Every directory holding a `Cargo.toml`, outside directories named `target`, reading and
    /// filling `memo` for committed subtrees.
    fn manifest_dirs(&self, memo: &mut ManifestDirs) -> Result<Vec<String>, Error>;
}

/// The directories holding a `Cargo.toml` below each committed tree, by tree id.
pub(crate) type ManifestDirs = HashMap<ObjectId, Rc<[String]>>;

/// The entries of each committed tree, by tree id.
pub(crate) type Trees = HashMap<ObjectId, Rc<Vec<TreeEntry>>>;

/// The tree object of `id`, an error when the object is absent or not a tree.
fn tree(repo: &gix::Repository, id: ObjectId) -> Result<gix::Tree<'_>, Error> {
    repo.find_object(id)
        .map(gix::Object::into_tree)
        .map_err(|source| Error::Object { id, source })
}

/// The directories holding a `Cargo.toml` below tree `id` of `repo`, relative to it.
fn manifests_below(
    repo: &gix::Repository,
    id: ObjectId,
    memo: &mut ManifestDirs,
) -> Result<Rc<[String]>, Error> {
    if let Some(found) = memo.get(&id) {
        return Ok(Rc::clone(found));
    }
    let tree = tree(repo, id)?;
    let mut found = Vec::new();
    let mut children = Vec::new();
    for entry in tree.iter() {
        let entry = entry.map_err(|source| Error::TreeList { id, source })?;
        let name = entry.filename().to_string();
        if entry.mode().is_tree() {
            if name != "target" {
                children.push((name, entry.object_id()));
            }
        } else if name == "Cargo.toml" {
            found.push(String::new());
        }
    }
    for (name, child) in children {
        for dir in manifests_below(repo, child, memo)?.iter() {
            found.push(if dir.is_empty() {
                name.clone()
            } else {
                format!("{name}/{dir}")
            });
        }
    }
    let found: Rc<[String]> = found.into();
    memo.insert(id, Rc::clone(&found));
    Ok(found)
}

/// The files of a committed tree.
pub(crate) struct TreeSnapshot<'r> {
    repo: &'r gix::Repository,
    tree: ObjectId,
    trees: Rc<RefCell<Trees>>,
}

impl<'r> TreeSnapshot<'r> {
    pub(crate) fn new(
        repo: &'r gix::Repository,
        tree: ObjectId,
        trees: &Rc<RefCell<Trees>>,
    ) -> Self {
        Self {
            repo,
            tree,
            trees: Rc::clone(trees),
        }
    }

    fn root(&self) -> Result<gix::Tree<'r>, Error> {
        tree(self.repo, self.tree)
    }

    /// The entry at `path`, following symlinks to targets inside the snapshot, at most 40 of
    /// them, as the kernel's limit is.
    fn entry(&self, path: &str) -> Result<Option<(ObjectId, gix::objs::tree::EntryMode)>, Error> {
        const HOPS: usize = 40;
        let mut found = None;
        let mut hops = 0;
        let mut targets: Vec<String> = Vec::new();
        let mut runs: Vec<(usize, Option<usize>)> = Vec::new();
        let mut pos: Option<usize> = Some(0);
        let mut trees: Vec<ObjectId> = Vec::with_capacity(path.split('/').count() + 1);
        trees.push(self.tree);
        loop {
            // Drop symlink targets whose segments are all consumed, newest first.
            while let Some((_, at)) = runs.last() {
                if at.is_some() {
                    break;
                }
                runs.pop();
                targets.pop();
            }
            let Some(name) = next_segment(path, &mut pos, &mut runs, &targets) else {
                break;
            };
            match name {
                "" | "." => continue,
                ".." => {
                    if trees.len() == 1 {
                        return Ok(None);
                    }
                    trees.pop();
                    found = None;
                    continue;
                }
                _ => {}
            }
            let dir = *trees.last().expect("the root tree is never popped");
            let Some((id, mode)) = self.child(dir, name)? else {
                return Ok(None);
            };
            if mode.is_link() {
                hops += 1;
                let target = self.read(name, id)?;
                if hops > HOPS {
                    return Ok(None);
                }
                let target = String::from_utf8_lossy(&target).into_owned();
                if target.starts_with('/') {
                    return Ok(None);
                }
                targets.push(target);
                runs.push((targets.len() - 1, Some(0)));
            } else if mode.is_tree() {
                trees.push(id);
                found = Some((id, mode));
            } else if runs.iter().all(|(_, at)| at.is_none()) && pos.is_none() {
                return Ok(Some((id, mode)));
            } else {
                return Ok(None);
            }
        }
        Ok(found)
    }

    /// The run's memo of tree `dir`'s parsed entries, decoded once when missing, a tree that
    /// fails to decode keeping its entries parsed before the failure with the error.
    fn memo(&self, dir: ObjectId) -> Result<Rc<Vec<TreeEntry>>, (Vec<TreeEntry>, Error)> {
        let mut memo = self.trees.borrow_mut();
        if let hashbrown::hash_map::Entry::Vacant(slot) = memo.entry(dir) {
            let entries = self.entries(dir)?;
            slot.insert(entries.into());
        }
        Ok(Rc::clone(
            memo.get(&dir).expect("the missing tree was just inserted"),
        ))
    }

    /// The entry called `name` in tree `dir`, from the run's memo of the tree's parsed entries.
    fn child(
        &self,
        dir: ObjectId,
        name: &str,
    ) -> Result<Option<(ObjectId, gix::objs::tree::EntryMode)>, Error> {
        let entries = match self.memo(dir) {
            // A tree that fails to decode answers the names before the failure, as reading
            // it up to the first match would.
            Err((entries, error)) => {
                return find_entry(&entries, name).map_or(Err(error), |found| Ok(Some(found)));
            }
            Ok(entries) => entries,
        };
        Ok(find_entry(&entries, name))
    }

    /// The parsed entries of tree `dir`, else those parsed before the first that fails, with
    /// the error.
    fn entries(&self, dir: ObjectId) -> Result<Vec<TreeEntry>, (Vec<TreeEntry>, Error)> {
        let tree = tree(self.repo, dir).map_err(|error| (Vec::new(), error))?;
        let mut entries = Vec::new();
        for entry in tree.iter() {
            match entry {
                Ok(entry) => entries.push((
                    entry.filename().to_string(),
                    entry.object_id(),
                    entry.mode(),
                )),
                Err(source) => return Err((entries, Error::TreeList { id: dir, source })),
            }
        }
        Ok(entries)
    }
}

/// One entry of a committed tree, its name, object and mode.
type TreeEntry = (String, ObjectId, gix::objs::tree::EntryMode);

/// The object and mode of the first of `entries` called `name`.
fn find_entry(entries: &[TreeEntry], name: &str) -> Option<(ObjectId, gix::objs::tree::EntryMode)> {
    entries
        .iter()
        .find(|(entry, _, _)| *entry == name)
        .map(|(_, id, mode)| (*id, *mode))
}

/// The next pending segment of `path` or of the newest symlink target in `runs`, a target
/// before the path it spliced into, advancing `pos` and the run's cursor past it, `None` when
/// nothing is pending.
fn next_segment<'s>(
    path: &'s str,
    pos: &mut Option<usize>,
    runs: &mut [(usize, Option<usize>)],
    targets: &'s [String],
) -> Option<&'s str> {
    if let Some((ti, at)) = runs.last_mut() {
        let p = (*at).expect("a pending run is never exhausted");
        let rest = &targets[*ti][p..];
        let (seg, next) = match rest.split_once('/') {
            Some((seg, _)) => (seg, Some(p + seg.len() + 1)),
            None => (rest, None),
        };
        *at = next;
        return Some(seg);
    }
    let p = (*pos)?;
    let rest = &path[p..];
    let (seg, next) = match rest.split_once('/') {
        Some((seg, _)) => (seg, Some(p + seg.len() + 1)),
        None => (rest, None),
    };
    *pos = next;
    Some(seg)
}

impl Snapshot for TreeSnapshot<'_> {
    fn blob(&self, path: &str) -> Result<Option<ObjectId>, Error> {
        Ok(self
            .entry(path)?
            .filter(|(_, mode)| mode.is_blob())
            .map(|(id, _)| id))
    }

    fn read(&self, _path: &str, id: ObjectId) -> Result<Vec<u8>, Error> {
        self.repo
            .find_object(id)
            .map(|object| object.detach().data)
            .map_err(|source| Error::Object { id, source })
    }

    fn list_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, Error> {
        let id = if dir.is_empty() {
            self.tree
        } else {
            match self.entry(dir)? {
                Some((id, mode)) if mode.is_tree() => id,
                _ => return Ok(Vec::new()),
            }
        };
        let entries = match self.memo(id) {
            Ok(entries) => entries,
            Err((_, error)) => return Err(error),
        };
        Ok(entries
            .iter()
            .map(|(name, _, mode)| (name.clone(), mode.is_tree()))
            .collect())
    }

    fn files(&self) -> Result<Vec<String>, Error> {
        let records = self
            .root()?
            .traverse()
            .breadthfirst
            .files()
            .map_err(|source| Error::TreeList {
                id: self.tree,
                source,
            })?;
        Ok(records
            .into_iter()
            .filter(|entry| entry.mode.is_blob())
            .map(|entry| entry.filepath.to_string())
            .collect())
    }

    fn manifest_dirs(&self, memo: &mut ManifestDirs) -> Result<Vec<String>, Error> {
        Ok(manifests_below(self.repo, self.tree, memo)?.to_vec())
    }
}

/// The files of the working tree, honouring ignore files.
pub(crate) struct WorkTreeSnapshot {
    root: PathBuf,
    skip: Vec<PathBuf>,
    hashes: RefCell<HashMap<String, Option<ObjectId>>>,
}

impl WorkTreeSnapshot {
    /// The working tree at `root`, never descending into the directories in `skip`.
    pub(crate) fn new(root: PathBuf, skip: Vec<PathBuf>) -> Self {
        Self {
            root,
            skip,
            hashes: RefCell::new(HashMap::new()),
        }
    }

    fn full(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl Snapshot for WorkTreeSnapshot {
    fn blob(&self, path: &str) -> Result<Option<ObjectId>, Error> {
        if let Some(&known) = self.hashes.borrow().get(path) {
            return Ok(known);
        }
        let full = self.full(path);
        let id = if path.is_empty() || !full.is_file() {
            None
        } else {
            let data = std::fs::read(&full).map_err(|source| Error::Io {
                path: full.clone(),
                source,
            })?;
            let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &data)
                .map_err(|source| Error::Hash {
                path: path.to_owned(),
                source,
            })?;
            Some(id)
        };
        self.hashes.borrow_mut().insert(path.to_owned(), id);
        Ok(id)
    }

    fn read(&self, path: &str, _id: ObjectId) -> Result<Vec<u8>, Error> {
        let full = self.full(path);
        std::fs::read(&full).map_err(|source| Error::Io { path: full, source })
    }

    fn list_dir(&self, dir: &str) -> Result<Vec<(String, bool)>, Error> {
        let full = self.full(dir);
        let Ok(entries) = std::fs::read_dir(&full) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: full.clone(),
                source,
            })?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            out.push((name, entry.path().is_dir()));
        }
        Ok(out)
    }

    fn files(&self) -> Result<Vec<String>, Error> {
        let skip = self.skip.clone();
        let walker = ignore::WalkBuilder::new(&self.root)
            .hidden(false)
            .parents(false)
            .require_git(false)
            .filter_entry(move |entry| {
                entry.file_name() != ".git" && !skip.iter().any(|dir| entry.path() == dir)
            })
            .build();
        let mut files = Vec::new();
        for entry in walker {
            let entry = entry.map_err(Error::WorkTree)?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            if let Some(relative) = relative(&self.root, entry.path()) {
                files.push(relative);
            }
        }
        files.sort();
        Ok(files)
    }

    fn manifest_dirs(&self, _memo: &mut ManifestDirs) -> Result<Vec<String>, Error> {
        Ok(self
            .files()?
            .iter()
            .filter_map(|file| match file.as_str() {
                "Cargo.toml" => Some(""),
                file => file.strip_suffix("/Cargo.toml"),
            })
            .filter(|dir| !dir.split('/').any(|part| part == "target"))
            .map(str::to_owned)
            .collect())
    }
}

/// `path` relative to `root` with `/` separators.
pub(crate) fn relative(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = rest.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// A commit of the scanned range.
pub(crate) struct Commit {
    pub(crate) id: ObjectId,
    pub(crate) tree: ObjectId,
    pub(crate) parents: Vec<ObjectId>,
}

/// What the range is compared against.
pub(crate) enum Head {
    /// The working tree, whose parent is `HEAD` when there is one.
    WorkTree(Option<ObjectId>),
    /// A commit.
    Commit(ObjectId),
}

/// The commits of `range`, children before parents, what they are compared against, and the
/// range's starting commit when it has one.
pub(crate) fn range(
    repo: &gix::Repository,
    range: &Range,
) -> Result<(Vec<Commit>, Head, Option<ObjectId>), Error> {
    let head_commit = || -> Option<ObjectId> { repo.head_id().ok().map(gix::Id::detach) };
    let (tip, hidden, head) = match range {
        Range::Full => (head_commit(), None, Head::WorkTree(head_commit())),
        Range::Since(since) => (
            head_commit(),
            Some(resolve(repo, since)?),
            Head::WorkTree(head_commit()),
        ),
        Range::Base { base, head } => {
            let base = resolve(repo, base)?;
            match head {
                Some(spec) => {
                    let tip = resolve(repo, spec)?;
                    (Some(tip), Some(base), Head::Commit(tip))
                }
                None => (head_commit(), Some(base), Head::WorkTree(head_commit())),
            }
        }
    };
    let Some(tip) = tip else {
        return Ok((Vec::new(), head, hidden));
    };
    let mut ids: Vec<(ObjectId, Vec<ObjectId>)> = Vec::new();
    let walk = repo
        .rev_walk([tip])
        .with_hidden(hidden)
        .all()
        .map_err(Error::Walk)?;
    for info in walk {
        let info = info.map_err(Error::Walk)?;
        ids.push((info.id, info.parent_ids().map(gix::Id::detach).collect()));
    }
    // The base's parents lie outside the range, where blame never looks.
    if let Some(base) = hidden
        && !ids.iter().any(|(id, _)| *id == base)
    {
        ids.push((base, Vec::new()));
    }
    let mut commits = Vec::with_capacity(ids.len());
    for (id, parents) in topological(ids) {
        let tree = commit_tree(repo, id)?;
        commits.push(Commit { id, tree, parents });
    }
    Ok((commits, head, hidden))
}

/// The root tree of commit `id`.
pub(crate) fn commit_tree(repo: &gix::Repository, id: ObjectId) -> Result<ObjectId, Error> {
    Ok(repo
        .find_commit(id)
        .map_err(|source| Error::Object { id, source })?
        .tree_id()
        .map_err(|source| Error::Object { id, source })?
        .detach())
}

fn resolve(repo: &gix::Repository, spec: &str) -> Result<ObjectId, Error> {
    let failed = |source| Error::Revision {
        spec: spec.to_owned(),
        source,
    };
    let object = repo
        .rev_parse_single(spec)
        .map_err(failed)?
        .object()
        .map_err(failed)?;
    Ok(object.peel_to_commit().map_err(failed)?.id)
}

/// Orders commits children first, keeping the walk order among independent commits.
fn topological(ids: Vec<(ObjectId, Vec<ObjectId>)>) -> Vec<(ObjectId, Vec<ObjectId>)> {
    let present: HashSet<ObjectId> = ids.iter().map(|(id, _)| *id).collect();
    let mut children: HashMap<ObjectId, usize> = HashMap::new();
    for (_, parents) in &ids {
        for parent in parents.iter().filter(|p| present.contains(*p)) {
            *children.entry(*parent).or_default() += 1;
        }
    }
    let position: HashMap<ObjectId, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let mut slots: Vec<Option<(ObjectId, Vec<ObjectId>)>> = ids.into_iter().map(Some).collect();
    let mut ready: std::collections::BTreeSet<usize> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| {
            slot.as_ref()
                .is_some_and(|(id, _)| !children.contains_key(id))
        })
        .map(|(i, _)| i)
        .collect();
    let mut order = Vec::with_capacity(slots.len());
    while let Some(i) = ready.pop_first() {
        let (id, parents) = slots[i].take().expect("each commit is ordered once");
        for parent in &parents {
            if let Some(count) = children.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(position[parent]);
                }
            }
        }
        order.push((id, parents));
    }
    order
}
