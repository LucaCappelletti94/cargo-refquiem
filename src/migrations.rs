//! The migration trees of a repository, whose files replay in order: diesel's
//! `<timestamp>_<name>/up.sql` directories, sqlx's `migrations/<version>_<desc>.sql` files and
//! refinery's `V<n>__<name>.sql` files.

use serde::Deserialize;

use crate::sql::{Change, Schema};

/// The names that the migrations of each tree, replayed in order, leave declared.
pub(crate) fn replay(mut migrations: Vec<(String, Order, Vec<Change>)>) -> Vec<String> {
    migrations.sort_by(|(tree, order, _), (other_tree, other_order, _)| {
        (tree, order).cmp(&(other_tree, other_order))
    });
    let mut names = Vec::new();
    let mut migrations = migrations.into_iter().peekable();
    while let Some((tree, _, mut changes)) = migrations.next() {
        let mut schema = Schema::default();
        loop {
            for change in changes {
                schema.apply(change);
            }
            match migrations.next_if(|(next, _, _)| *next == tree) {
                Some((_, _, next)) => changes = next,
                None => break,
            }
        }
        names.extend(schema.into_names());
    }
    names
}

/// What a `.sql` file is to the migration trees.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// A file outside every migration layout, whose names stay alive whatever drops them.
    Plain,
    /// A migration of `tree`, replayed at `order`.
    Up { tree: String, order: Order },
    /// A migration undoing another, which declares nothing.
    Down,
}

/// Where a migration falls in its tree, by version number and then by name.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Order {
    /// How many digits the version has once its leading zeros go, so versions of any length
    /// compare as numbers.
    digits: usize,
    version: String,
    name: String,
}

impl Order {
    fn named(name: &str) -> Self {
        Self {
            digits: 0,
            version: String::new(),
            name: name.to_owned(),
        }
    }

    fn versioned(version: &str, name: &str) -> Self {
        let version = version.trim_start_matches('0');
        Self {
            digits: version.len(),
            version: version.to_owned(),
            name: name.to_owned(),
        }
    }
}

/// The migration directories `diesel.toml` files name, beside the layouts found by shape.
#[derive(Debug, Default)]
pub(crate) struct Layouts {
    diesel: Vec<String>,
}

impl Layouts {
    /// Records the `migrations_directory` that the `diesel.toml` at `file`, holding `source`,
    /// names. A file diesel could not read names none.
    pub(crate) fn diesel_toml(&mut self, file: &str, source: &str) {
        #[derive(Deserialize)]
        struct Config {
            migrations_directory: Option<Directory>,
        }
        #[derive(Deserialize)]
        struct Directory {
            dir: String,
        }
        let Ok(Config {
            migrations_directory: Some(Directory { dir }),
        }) = toml::from_str(source)
        else {
            return;
        };
        if let Some(dir) = join(split(file).0, &dir) {
            self.diesel.push(dir);
        }
    }

    /// The role of the `.sql` file at `file`.
    pub(crate) fn role(&self, file: &str) -> Role {
        let (dir, name) = split(file);
        let (tree, entry) = split(dir);
        if matches!(name, "up.sql" | "down.sql")
            && (is_diesel_entry(entry)
                || (!entry.is_empty() && self.diesel.iter().any(|d| d == tree)))
        {
            return if name == "up.sql" {
                Role::Up {
                    tree: tree.to_owned(),
                    order: Order::named(entry),
                }
            } else {
                Role::Down
            };
        }
        if split(dir).1 == "migrations"
            && let Some(version) = version(name, "_")
        {
            return if name.ends_with(".down.sql") {
                Role::Down
            } else {
                Role::Up {
                    tree: dir.to_owned(),
                    order: Order::versioned(version, name),
                }
            };
        }
        if let Some(version) = name.strip_prefix('V').and_then(|rest| version(rest, "__")) {
            return Role::Up {
                tree: dir.to_owned(),
                order: Order::versioned(version, name),
            };
        }
        Role::Plain
    }
}

/// The directory and the last part of `path`.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// Whether the directory `entry` is named as diesel names a migration, a version of digits and
/// dashes, an underscore and a name.
fn is_diesel_entry(entry: &str) -> bool {
    entry.split_once('_').is_some_and(|(version, name)| {
        version.starts_with(|c: char| c.is_ascii_digit())
            && version.chars().all(|c| c.is_ascii_digit() || c == '-')
            && !name.is_empty()
    })
}

/// The digits `name` starts with when `separator` and a description follow them, before the
/// lowercase `.sql` that sqlx and refinery require.
fn version<'a>(name: &'a str, separator: &str) -> Option<&'a str> {
    let (version, rest) = name.split_once(separator)?;
    let description = rest.strip_suffix(".sql")?;
    (!version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) && !description.is_empty())
        .then_some(version)
}

/// `relative` resolved against the directory `base` of the repository, when it stays inside
/// the repository.
fn join(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}
