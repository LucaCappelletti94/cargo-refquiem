//! The names SQL declares and the changes it makes to a schema, read with `sqlparser`, and the
//! SQL it could not read.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, ArgMode, CreateFunction, CreateFunctionBody, DataType, DoOption,
    DoStatement, Expr, FunctionDesc, Ident, ObjectName, ObjectNamePart, ObjectType,
    RenameTableNameKind, SchemaName, Statement, TableConstraint, UserDefinedTypeRepresentation,
    Value, ValueWithSpan, Visit, Visitor,
};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};
use sqlparser_plpgsql::embedded::{Block, Branch, Node};

use crate::HashMap;

/// How many `DO` and function bodies may nest inside one another before the innermost is
/// refused, which bounds the recursion a body of bodies drives.
const BODY_DEPTH: usize = 8;

/// A dialect SQL is read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Dialect {
    Generic,
    #[serde(alias = "postgresql", alias = "pg")]
    Postgres,
    #[serde(alias = "mariadb")]
    Mysql,
    Sqlite,
}

impl Dialect {
    /// The dialect the nearest directory of `file` names, such as `postgres` in diesel's
    /// `migrations/postgres/…/up.sql`.
    pub(crate) fn of_path(file: &str) -> Option<Self> {
        file.rsplit('/').skip(1).find_map(Self::named)
    }

    /// The dialect a directory or fence language spells, in any case.
    pub(crate) fn named(name: &str) -> Option<Self> {
        let is = |names: &[&str]| names.iter().any(|known| name.eq_ignore_ascii_case(known));
        if is(&["postgres", "postgresql", "pg"]) {
            Some(Self::Postgres)
        } else if is(&["mysql", "mariadb"]) {
            Some(Self::Mysql)
        } else if is(&["sqlite"]) {
            Some(Self::Sqlite)
        } else {
            None
        }
    }

    fn parser(self) -> &'static dyn sqlparser::dialect::Dialect {
        match self {
            Self::Generic => &GenericDialect,
            Self::Postgres => &PostgreSqlDialect {},
            Self::Mysql => &MySqlDialect {},
            Self::Sqlite => &SQLiteDialect {},
        }
    }
}

/// SQL that could not be read, at a one-based line and character column of its document.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) error: String,
}

/// The changes a SQL document makes to the schema, in the order it makes them, and the parts of
/// it that could not be read.
#[derive(Debug, Default)]
pub(crate) struct Declared {
    pub(crate) changes: Vec<Change>,
    pub(crate) refused: Vec<Refusal>,
}

/// What one statement does to the schema.
#[derive(Debug)]
pub(crate) enum Change {
    /// Creates `object`, which dies with `owner` when it has one, keeping `names` alive.
    Create {
        object: Key,
        owner: Option<Key>,
        names: Vec<String>,
    },
    /// Removes the object and everything it owns.
    Drop(Key),
    /// Removes every overload of the routine of this name and everything they own.
    DropRoutines(Vec<String>),
    /// Turns `from` into `to`, owned by `owner` and keeping `names` alive, and moves what `from`
    /// owns to `to`.
    Rename {
        from: Key,
        to: Key,
        owner: Option<Key>,
        names: Vec<String>,
    },
}

/// The names `changes` declare, whatever they later drop, as a document outside a migration
/// tree keeps them.
pub(crate) fn names(changes: Vec<Change>) -> impl Iterator<Item = String> {
    changes.into_iter().flat_map(|change| match change {
        Change::Create { names, .. } | Change::Rename { names, .. } => names,
        Change::Drop(_) | Change::DropRoutines(_) => Vec::new(),
    })
}

/// An object of the schema, as its name is written, each unquoted part folded to lowercase.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    space: Space,
    /// The parts of its name, after those of the object it belongs to for a column,
    /// constraint, trigger, policy, type member or parameter, and ending in the argument types
    /// for a routine.
    path: Vec<String>,
}

/// The kinds of object whose names never meet, as a table and a role may share a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Space {
    /// Tables, views, indexes and sequences, which PostgreSQL names alike.
    Relation,
    Column,
    Constraint,
    Trigger,
    Policy,
    /// Types and domains.
    Type,
    /// Enum labels and composite attributes.
    Member,
    /// Functions and procedures.
    Routine,
    Parameter,
    /// Roles and users.
    Role,
    Schema,
    Extension,
}

impl Key {
    fn of(space: Space, name: &ObjectName) -> Self {
        let path = name.0.iter().map(part).collect();
        Self { space, path }
    }

    fn bare(space: Space, ident: &Ident) -> Self {
        Self {
            space,
            path: vec![fold(ident)],
        }
    }

    /// The object `ident` of `owner`, such as a column of a table.
    fn under(space: Space, owner: &Self, ident: &Ident) -> Self {
        let mut path = owner.path.clone();
        path.push(fold(ident));
        Self { space, path }
    }

    /// The object of `owner` the last part of `name` names.
    fn under_last(space: Space, owner: &Self, name: &ObjectName) -> Self {
        let mut path = owner.path.clone();
        path.extend(name.0.last().map(part));
        Self { space, path }
    }

    /// The routine `name` taking arguments of `types`, which PostgreSQL tells overloads apart
    /// by.
    fn routine<'a>(name: &ObjectName, types: impl Iterator<Item = &'a DataType>) -> Self {
        let mut key = Self::of(Space::Routine, name);
        let types: Vec<String> = types.map(|kind| kind.to_string().to_lowercase()).collect();
        key.path.push(format!("({})", types.join(",")));
        key
    }
}

/// A part of an object's name, folded when it is an identifier.
fn part(part: &ObjectNamePart) -> String {
    part.as_ident().map_or_else(|| part.to_string(), fold)
}

/// `ident` as an object's name, lowercased as PostgreSQL folds it when unquoted.
fn fold(ident: &Ident) -> String {
    if ident.quote_style.is_none() {
        ident.value.to_lowercase()
    } else {
        ident.value.clone()
    }
}

/// The names `ident` keeps alive, as written and lowercased as PostgreSQL folds it when
/// unquoted.
fn spellings(ident: &Ident) -> Vec<String> {
    let folded = fold(ident);
    if folded == ident.value {
        vec![folded]
    } else {
        vec![folded, ident.value.clone()]
    }
}

/// The last part of `name`, which is the object a qualified name declares. Only Snowflake's
/// `IDENTIFIER(…)` ends a name in something else, which no dialect read here parses.
fn last(name: &ObjectName) -> Option<&Ident> {
    name.0.last().and_then(ObjectNamePart::as_ident)
}

/// The objects a migration tree leaves alive, replayed change by change.
#[derive(Debug, Default)]
pub(crate) struct Schema {
    objects: HashMap<Key, Object>,
}

/// A live object, the object it dies with and the names it keeps alive. An object with an
/// owner owns nothing itself.
#[derive(Debug)]
struct Object {
    owner: Option<Key>,
    names: Vec<String>,
}

impl Schema {
    pub(crate) fn apply(&mut self, change: Change) {
        match change {
            Change::Create {
                object,
                owner,
                names,
            } => self
                .objects
                .entry(object)
                .or_insert(Object {
                    owner,
                    names: Vec::new(),
                })
                .names
                .extend(names),
            Change::Drop(key) => self.remove(|candidate| *candidate == key),
            Change::DropRoutines(name) => self.remove(|candidate| {
                candidate.space == Space::Routine
                    && candidate
                        .path
                        .split_last()
                        .is_some_and(|(_, path)| *path == name)
            }),
            Change::Rename {
                from,
                to,
                owner,
                names,
            } => {
                self.objects.remove(&from);
                let carried: Vec<(Key, Object)> = self
                    .objects
                    .extract_if(|_, object| object.owner.as_ref() == Some(&from))
                    .collect();
                for (key, mut object) in carried {
                    object.owner = Some(to.clone());
                    let key = match key.path.strip_prefix(from.path.as_slice()) {
                        Some(rest) => Key {
                            space: key.space,
                            path: to.path.iter().chain(rest).cloned().collect(),
                        },
                        None => key,
                    };
                    self.objects.insert(key, object);
                }
                self.objects.insert(to, Object { owner, names });
            }
        }
    }

    /// Removes every object `dropped` picks and every object whose owner it picks.
    fn remove(&mut self, dropped: impl Fn(&Key) -> bool) {
        self.objects
            .retain(|key, object| !(dropped(key) || object.owner.as_ref().is_some_and(&dropped)));
    }

    /// The names the live objects keep alive.
    pub(crate) fn into_names(self) -> impl Iterator<Item = String> {
        self.objects.into_values().flat_map(|object| object.names)
    }
}

/// Everything `source`, read in `dialect`, declares and changes, `DO` and function bodies
/// included. A document sqlparser refuses declares nothing, and a refused body loses only its
/// own names.
pub(crate) fn declared(source: &str, dialect: Dialect) -> Declared {
    collect(tokenize(source, dialect), dialect).0
}

/// What the `.sql` file `source`, read in `dialect`, declares, and its comments when sqlparser
/// reads it, a refused file having none.
pub(crate) fn read(source: &str, dialect: Dialect) -> (Declared, Comments) {
    let tokens = tokenize(source, dialect);
    let mut comments = Comments::default();
    if let Ok(tokens) = &tokens {
        comments.gather(source, 0, tokens, dialect, 0);
    }
    let (declared, parsed) = collect(tokens, dialect);
    (
        declared,
        if parsed {
            comments
        } else {
            Comments::default()
        },
    )
}

/// The names `tokens` declare, and whether sqlparser parsed them.
fn collect(tokens: Result<Vec<TokenWithSpan>, Refusal>, dialect: Dialect) -> (Declared, bool) {
    let mut collector = Collector {
        dialect,
        depth: 0,
        enclosing: 0,
        body_at: None,
        out: Declared::default(),
    };
    let parsed = match tokens.and_then(|tokens| statements(tokens, dialect)) {
        Ok(statements) => {
            collector.statements(&statements);
            true
        }
        Err(refusal) => {
            collector.out.refused.push(refusal);
            false
        }
    };
    (collector.out, parsed)
}

/// The statements of `source`, tokenized first so a tokenizer error keeps its location.
fn parse(source: &str, dialect: Dialect) -> Result<Vec<Statement>, Refusal> {
    statements(tokenize(source, dialect)?, dialect)
}

fn tokenize(source: &str, dialect: Dialect) -> Result<Vec<TokenWithSpan>, Refusal> {
    Tokenizer::new(dialect.parser(), source)
        .tokenize_with_location()
        .map_err(|error| Refusal {
            line: number(error.location.line),
            column: number(error.location.column),
            error: error.message,
        })
}

fn statements(tokens: Vec<TokenWithSpan>, dialect: Dialect) -> Result<Vec<Statement>, Refusal> {
    Parser::new(dialect.parser())
        .with_tokens_with_locations(tokens)
        .parse_statements()
        .map_err(|error| refusal(&error))
}

/// Whether comment text `body` is SQL statements rather than prose.
fn is_sql(body: &str, dialect: Dialect) -> bool {
    parse(body, dialect).is_ok_and(|statements| !statements.is_empty())
}

/// A comment of a SQL document as byte ranges of the document, the `--` comments of
/// consecutive lines joined into one run.
#[derive(Debug)]
pub(crate) struct Run {
    pub(crate) lines: Vec<(usize, usize)>,
    /// Whether the run holds nothing but SQL statements.
    pub(crate) code: bool,
}

/// The comments of a SQL document and the text its `COMMENT ON … IS` statements attach, as
/// byte ranges of the document, those inside dollar-quoted bodies included.
#[derive(Debug, Default)]
pub(crate) struct Comments {
    pub(crate) runs: Vec<Run>,
    pub(crate) documentation: Vec<(usize, usize)>,
}

/// Where a `COMMENT ON … IS` statement stands as its tokens are read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Documenting {
    No,
    Comment,
    On,
    Is,
}

impl Comments {
    /// Adds the comments of `tokens`, the tokens of `text`, which starts at byte `base` of the
    /// document, and those of the dollar-quoted bodies they hold, `depth` bodies deep.
    fn gather(
        &mut self,
        text: &str,
        base: usize,
        tokens: &[TokenWithSpan],
        dialect: Dialect,
        depth: usize,
    ) {
        let starts = crate::text::line_starts(text);
        let at = |location: Location| base + byte_offset(text, &starts, location);
        let mut lines: Vec<(usize, usize)> = Vec::new();
        // The line breaks since the last `--` comment, one of which ends its line and a second a run.
        let mut breaks = 0;
        let mut documenting = Documenting::No;
        for (i, token) in tokens.iter().enumerate() {
            let start = at(token.span.start);
            let end = tokens
                .get(i + 1)
                .map_or(base + text.len(), |next| at(next.span.start));
            match &token.token {
                Token::Whitespace(Whitespace::SingleLineComment { .. }) => {
                    let line = text[start - base..end - base].trim_end_matches(['\n', '\r']);
                    lines.push((start, start + line.len()));
                    breaks = 0;
                    continue;
                }
                Token::Whitespace(Whitespace::Space | Whitespace::Tab) => continue,
                Token::Whitespace(Whitespace::MultiLineComment(body)) => {
                    self.close(text, base, &mut lines, dialect);
                    self.runs.push(Run {
                        lines: vec![(start, end)],
                        code: is_sql(body, dialect),
                    });
                    continue;
                }
                Token::Whitespace(Whitespace::Newline) => {
                    breaks += 1;
                    if breaks < 2 {
                        continue;
                    }
                }
                Token::DollarQuotedString(string) => {
                    let open = string.tag.as_ref().map_or(0, String::len) + 2;
                    if depth < BODY_DEPTH
                        && let Ok(body) = tokenize(&string.value, dialect)
                    {
                        self.gather(&string.value, start + open, &body, dialect, depth + 1);
                    }
                    documenting = self.document(documenting, start, end);
                }
                Token::SingleQuotedString(_) | Token::EscapedStringLiteral(_) => {
                    documenting = self.document(documenting, start, end);
                }
                Token::Word(word) => {
                    documenting = match (documenting, word.keyword) {
                        (_, Keyword::COMMENT) => Documenting::Comment,
                        (Documenting::On, Keyword::IS) => Documenting::Is,
                        (Documenting::On, _) | (Documenting::Comment, Keyword::ON) => {
                            Documenting::On
                        }
                        _ => Documenting::No,
                    };
                }
                Token::SemiColon => documenting = Documenting::No,
                _ if documenting == Documenting::On => {}
                _ => documenting = Documenting::No,
            }
            self.close(text, base, &mut lines, dialect);
        }
        self.close(text, base, &mut lines, dialect);
    }

    /// Records the string literal at `start..end` when it is the text a `COMMENT ON … IS`
    /// attaches. PostgreSQL names the object commented on without strings, so any string ends
    /// the statement's reading.
    fn document(&mut self, documenting: Documenting, start: usize, end: usize) -> Documenting {
        if documenting == Documenting::Is {
            self.documentation.push((start, end));
        }
        Documenting::No
    }

    /// Ends the run of `--` comments `lines` of `text`, which starts at byte `base`.
    fn close(
        &mut self,
        text: &str,
        base: usize,
        lines: &mut Vec<(usize, usize)>,
        dialect: Dialect,
    ) {
        if lines.is_empty() {
            return;
        }
        let body: Vec<&str> = lines
            .iter()
            .map(|&(start, end)| {
                let line = &text[start - base..end - base];
                line.strip_prefix("--")
                    .or_else(|| line.strip_prefix('#'))
                    .unwrap_or(line)
            })
            .collect();
        self.runs.push(Run {
            lines: std::mem::take(lines),
            code: is_sql(&body.join("\n"), dialect),
        });
    }
}

/// The byte offset in `text`, whose lines start at `starts`, of a sqlparser location.
fn byte_offset(text: &str, starts: &[usize], location: Location) -> usize {
    let line = number(location.line).saturating_sub(1);
    let line_start = starts.get(line).copied().unwrap_or(text.len());
    text[line_start..]
        .char_indices()
        .nth(number(location.column).saturating_sub(1))
        .map_or(text.len(), |(at, _)| line_start + at)
}

/// `error` as a refusal at the location its message ends with, sqlparser's ` at Line: L,
/// Column: C`, the first line otherwise.
fn refusal(error: &ParserError) -> Refusal {
    let message = match error {
        ParserError::TokenizerError(message) | ParserError::ParserError(message) => message,
        ParserError::RecursionLimitExceeded => "recursion limit exceeded",
    };
    let located = message.rsplit_once(" at Line: ").and_then(|(text, at)| {
        let (line, column) = at.split_once(", Column: ")?;
        Some((text, line.parse().ok()?, column.parse().ok()?))
    });
    let (text, line, column) = located.unwrap_or((message, 1, 1));
    Refusal {
        line,
        column,
        error: text.to_owned(),
    }
}

/// A one-based sqlparser line or column as a `usize`, saturating where it cannot fit.
fn number(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// The byte offset of one-based `line` and character `column` in `text`, its end when past it.
pub(crate) fn offset(text: &str, line: usize, column: usize) -> usize {
    let line_start = if line > 1 {
        text.match_indices('\n')
            .nth(line - 2)
            .map_or(text.len(), |(at, _)| at + 1)
    } else {
        0
    };
    text[line_start..]
        .char_indices()
        .nth(column.saturating_sub(1))
        .map_or(text.len(), |(at, _)| line_start + at)
}

/// Gathers the changes statements make, reading the bodies they hold.
struct Collector {
    dialect: Dialect,
    /// How many bodies enclose the statements being read, which bounds their nesting.
    depth: usize,
    /// How many statements enclose the one being read. A statement inside a body or another
    /// statement, such as a procedure's, only declares, since it runs when its holder is called
    /// or behind conditions read at run time.
    enclosing: usize,
    /// Where the outermost body being read starts in the document, which every refusal inside
    /// it is reported at.
    body_at: Option<Location>,
    out: Declared,
}

impl Visitor for Collector {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        self.statement(statement);
        self.enclosing += 1;
        ControlFlow::Continue(())
    }

    fn post_visit_statement(&mut self, _: &Statement) -> ControlFlow<()> {
        self.enclosing -= 1;
        ControlFlow::Continue(())
    }
}

impl Collector {
    /// Gathers the changes of every statement in `tree`, nested ones included.
    fn statements<V: Visit>(&mut self, tree: &V) {
        let _ = tree.visit(self);
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::CreateTable(table) => {
                let key = self.object(Space::Relation, &table.name, None);
                for column in &table.columns {
                    self.member(Space::Column, &key, &column.name);
                }
                for constraint in &table.constraints {
                    self.constraint(&key, constraint);
                }
            }
            Statement::CreateVirtualTable { name, .. } | Statement::CreateSequence { name, .. } => {
                self.object(Space::Relation, name, None);
            }
            Statement::CreateTrigger(trigger) => {
                let key = self.trigger(&trigger.name, Some(&trigger.table_name));
                let table = Key::of(Space::Relation, &trigger.table_name);
                self.create(key, Some(table), last(&trigger.name).map(spellings));
            }
            Statement::CreateView(view) => {
                let key = self.object(Space::Relation, &view.name, None);
                for column in &view.columns {
                    self.member(Space::Column, &key, &column.name);
                }
            }
            Statement::CreateIndex(index) => {
                if let Some(name) = &index.name {
                    let table = Key::of(Space::Relation, &index.table_name);
                    let key = self.index(&table, name);
                    self.create(key, Some(table), last(name).map(spellings));
                }
            }
            Statement::CreateRole(role) => {
                for name in &role.names {
                    self.object(Space::Role, name, None);
                }
            }
            Statement::CreateUser(user) => self.bare(Space::Role, &user.name, None),
            Statement::CreatePolicy(policy) => {
                let table = Key::of(Space::Relation, &policy.table_name);
                self.member(Space::Policy, &table, &policy.name);
            }
            Statement::CreateExtension(extension) => {
                self.bare(Space::Extension, &extension.name, None);
            }
            Statement::CreateSchema { schema_name, .. } => match schema_name {
                SchemaName::Simple(name) | SchemaName::NamedAuthorization(name, _) => {
                    self.object(Space::Schema, name, None);
                }
                SchemaName::UnnamedAuthorization(ident) => self.bare(Space::Schema, ident, None),
            },
            Statement::CreateFunction(function) => self.function(function),
            Statement::CreateProcedure { name, params, .. } => {
                let params = params.iter().flatten();
                let inputs = params
                    .clone()
                    .filter(|param| param.mode != Some(ArgMode::Out));
                let key = Key::routine(name, inputs.map(|param| &param.data_type));
                self.create(key.clone(), None, last(name).map(spellings));
                for param in params {
                    self.member(Space::Parameter, &key, &param.name);
                }
            }
            Statement::CreateDomain(domain) => {
                let key = self.object(Space::Type, &domain.name, None);
                for constraint in &domain.constraints {
                    self.constraint(&key, constraint);
                }
            }
            Statement::CreateType {
                name,
                representation,
            } => {
                let key = self.object(Space::Type, name, None);
                match representation {
                    Some(UserDefinedTypeRepresentation::Composite { attributes }) => {
                        for attribute in attributes {
                            self.member(Space::Member, &key, &attribute.name);
                        }
                    }
                    Some(UserDefinedTypeRepresentation::Enum { labels }) => {
                        for label in labels {
                            self.member(Space::Member, &key, label);
                        }
                    }
                    _ => {}
                }
            }
            Statement::AlterTable(alter) => {
                let table = Key::of(Space::Relation, &alter.name);
                for operation in &alter.operations {
                    self.alteration(&table, operation);
                }
            }
            Statement::Do(block) => self.do_block(block),
            statement => self.drop(statement),
        }
    }

    /// Records what a `DROP` statement removes.
    fn drop(&mut self, statement: &Statement) {
        match statement {
            Statement::Drop {
                object_type,
                names,
                table,
                ..
            } => {
                let space = match object_type {
                    ObjectType::Table
                    | ObjectType::ForeignTable
                    | ObjectType::View
                    | ObjectType::MaterializedView
                    | ObjectType::Index
                    | ObjectType::Sequence => Space::Relation,
                    ObjectType::Schema => Space::Schema,
                    ObjectType::Role | ObjectType::User | ObjectType::Group => Space::Role,
                    ObjectType::Type => Space::Type,
                    _ => return,
                };
                for name in names {
                    let key = match (object_type, table) {
                        (ObjectType::Index, Some(table)) => {
                            self.index(&Key::of(Space::Relation, table), name)
                        }
                        _ => Key::of(space, name),
                    };
                    self.remove(Change::Drop(key));
                }
            }
            Statement::DropFunction(drop) => self.drop_routines(&drop.func_desc),
            Statement::DropProcedure { proc_desc, .. } => self.drop_routines(proc_desc),
            Statement::DropDomain(drop) => {
                self.remove(Change::Drop(Key::of(Space::Type, &drop.name)));
            }
            Statement::DropPolicy(drop) => {
                let table = Key::of(Space::Relation, &drop.table_name);
                self.remove(Change::Drop(Key::under(Space::Policy, &table, &drop.name)));
            }
            Statement::DropExtension(drop) => {
                for name in &drop.names {
                    self.remove(Change::Drop(Key::bare(Space::Extension, name)));
                }
            }
            Statement::DropTrigger(drop) => {
                let key = self.trigger(&drop.trigger_name, drop.table_name.as_ref());
                self.remove(Change::Drop(key));
            }
            _ => {}
        }
    }

    fn alteration(&mut self, table: &Key, operation: &AlterTableOperation) {
        let renamed = |space, old, new| {
            (
                Key::under(space, table, old),
                Key::under(space, table, new),
                spellings(new),
            )
        };
        let (from, to, names) = match operation {
            AlterTableOperation::AddColumn { column_def, .. } => {
                self.member(Space::Column, table, &column_def.name);
                return;
            }
            AlterTableOperation::AddConstraint { constraint, .. } => {
                self.constraint(table, constraint);
                return;
            }
            AlterTableOperation::DropColumn { column_names, .. } => {
                for name in column_names {
                    self.remove(Change::Drop(Key::under(Space::Column, table, name)));
                }
                return;
            }
            AlterTableOperation::DropConstraint { name, .. } => {
                self.remove(Change::Drop(Key::under(Space::Constraint, table, name)));
                return;
            }
            // MySQL drops an index, or the key that declared it, by its name.
            AlterTableOperation::DropIndex { name } => {
                self.remove(Change::Drop(self.index_named(table, name)));
                self.remove(Change::Drop(Key::under(Space::Constraint, table, name)));
                return;
            }
            AlterTableOperation::RenameColumn {
                old_column_name,
                new_column_name,
            } => renamed(Space::Column, old_column_name, new_column_name),
            AlterTableOperation::ChangeColumn {
                old_name, new_name, ..
            } => renamed(Space::Column, old_name, new_name),
            AlterTableOperation::RenameConstraint { old_name, new_name } => {
                renamed(Space::Constraint, old_name, new_name)
            }
            AlterTableOperation::RenameTable {
                table_name: RenameTableNameKind::As(name) | RenameTableNameKind::To(name),
            } => {
                // A bare new name keeps the table in its schema.
                let to = if let [new] = name.0.as_slice() {
                    let mut to = table.clone();
                    to.path.pop();
                    to.path.push(part(new));
                    to
                } else {
                    Key::of(Space::Relation, name)
                };
                let names = last(name).map(spellings).unwrap_or_default();
                self.rename(table.clone(), to, None, names);
                return;
            }
            _ => return,
        };
        self.rename(from, to, Some(table.clone()), names);
    }

    /// The trigger `name` on `table`. PostgreSQL names a trigger within its table, while MySQL
    /// and SQLite name it within the schema and drop it without naming the table.
    fn trigger(&self, name: &ObjectName, table: Option<&ObjectName>) -> Key {
        match (self.dialect, table) {
            (Dialect::Postgres | Dialect::Generic, Some(table)) => {
                Key::under_last(Space::Trigger, &Key::of(Space::Relation, table), name)
            }
            _ => Key::of(Space::Trigger, name),
        }
    }

    /// The index `name` on `table`. MySQL names an index within its table, like the key that
    /// declares it, while PostgreSQL and SQLite name it within the schema.
    fn index(&self, table: &Key, name: &ObjectName) -> Key {
        if self.dialect == Dialect::Mysql {
            Key::under_last(Space::Constraint, table, name)
        } else {
            Key::of(Space::Relation, name)
        }
    }

    /// The index `ident` on `table`, named as [`Collector::index`] names it.
    fn index_named(&self, table: &Key, ident: &Ident) -> Key {
        if self.dialect == Dialect::Mysql {
            Key::under(Space::Constraint, table, ident)
        } else {
            Key::bare(Space::Relation, ident)
        }
    }

    /// The constraint's name and the name of an index it declares, such as MySQL's keys.
    fn constraint(&mut self, owner: &Key, constraint: &TableConstraint) {
        let (name, index) = match constraint {
            TableConstraint::Unique(unique) => (&unique.name, &unique.index_name),
            TableConstraint::PrimaryKey(key) => (&key.name, &key.index_name),
            TableConstraint::ForeignKey(key) => (&key.name, &key.index_name),
            TableConstraint::Check(check) => (&check.name, &None),
            TableConstraint::Index(index) => (&None, &index.name),
            TableConstraint::FulltextOrSpatial(index) => (&None, &index.opt_index_name),
            TableConstraint::PrimaryKeyUsingIndex(index)
            | TableConstraint::UniqueUsingIndex(index) => (&index.name, &None),
            TableConstraint::Exclude(exclude) => (&exclude.name, &None),
        };
        if let Some(name) = name {
            self.member(Space::Constraint, owner, name);
        }
        if let Some(index) = index {
            let key = self.index_named(owner, index);
            self.create(key, Some(owner.clone()), Some(spellings(index)));
        }
    }

    fn function(&mut self, function: &CreateFunction) {
        let args = function.args.iter().flatten();
        let inputs = args.clone().filter(|arg| arg.mode != Some(ArgMode::Out));
        let key = Key::routine(&function.name, inputs.map(|arg| &arg.data_type));
        self.create(key.clone(), None, last(&function.name).map(spellings));
        for name in args.filter_map(|arg| arg.name.as_ref()) {
            self.member(Space::Parameter, &key, name);
        }
        let body = match &function.function_body {
            Some(
                CreateFunctionBody::AsBeforeOptions {
                    body: Expr::Value(body),
                    ..
                }
                | CreateFunctionBody::AsAfterOptions(Expr::Value(body)),
            ) => source(body),
            _ => None,
        };
        let Some((text, at)) = body else {
            return;
        };
        let language = function
            .language
            .as_ref()
            .map(|language| language.value.as_str());
        self.body(language.unwrap_or("sql"), text, at);
    }

    /// Drops the routine each of `routines` names, every overload when it lists no arguments.
    fn drop_routines(&mut self, routines: &[FunctionDesc]) {
        for routine in routines {
            let change = match &routine.args {
                Some(args) => {
                    let inputs = args.iter().filter(|arg| arg.mode != Some(ArgMode::Out));
                    Change::Drop(Key::routine(
                        &routine.name,
                        inputs.map(|arg| &arg.data_type),
                    ))
                }
                None => Change::DropRoutines(Key::of(Space::Routine, &routine.name).path),
            };
            self.remove(change);
        }
    }

    fn do_block(&mut self, block: &DoStatement) {
        let mut language = "plpgsql";
        let mut code = None;
        for option in &block.options {
            match option {
                DoOption::Language(name) => language = &name.value,
                DoOption::LanguageLiteral(name) => language = text(&name.value).unwrap_or(language),
                DoOption::Code(body) => code = source(body),
            }
        }
        if let Some((text, at)) = code {
            self.body(language, text, at);
        }
    }

    /// Reads the body `source` written in `language`, which starts at `start` in the document,
    /// `plpgsql` through `sqlparser-plpgsql` and `sql` as statements of the document's dialect.
    fn body(&mut self, language: &str, source: &str, start: Location) {
        let plpgsql = language.eq_ignore_ascii_case("plpgsql");
        if !plpgsql && !language.eq_ignore_ascii_case("sql") {
            return;
        }
        let at = *self.body_at.get_or_insert(start);
        if self.depth == BODY_DEPTH {
            self.refuse(at, format!("bodies nest deeper than {BODY_DEPTH}"));
            return;
        }
        let outermost = self.depth == 0;
        self.depth += 1;
        if plpgsql {
            match sqlparser_plpgsql::embedded_sql(source) {
                Ok(block) => self.block(&block),
                Err(error) => self.refuse(at, error.to_string()),
            }
        } else {
            match parse(source, self.dialect) {
                Ok(statements) => self.statements(&statements),
                Err(refusal) => self.refuse(at, refusal.error),
            }
        }
        self.depth -= 1;
        if outermost {
            self.body_at = None;
        }
    }

    fn refuse(&mut self, at: Location, error: String) {
        self.out.refused.push(Refusal {
            line: number(at.line),
            column: number(at.column),
            error,
        });
    }

    fn block(&mut self, block: &Block) {
        self.statements(&block.declarations);
        self.nodes(&block.statements);
        for handler in &block.handlers {
            self.nodes(handler);
        }
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Statement(statement) => self.statements(&**statement),
                Node::Block(block) => self.block(block),
                Node::If(branching) => {
                    self.branches(&branching.branches);
                    self.nodes(&branching.otherwise);
                }
                Node::Case(case) => {
                    self.branches(&case.branches);
                    self.nodes(&case.otherwise);
                }
                Node::Loop(looped) => self.nodes(&looped.statements),
                // SQL the body builds at run time has no text to read.
                Node::Dynamic(_) => {}
            }
        }
    }

    fn branches(&mut self, branches: &[Branch]) {
        for branch in branches {
            self.nodes(&branch.statements);
        }
    }

    /// Creates the object `name` declares, owned by `owner`, and returns its key.
    fn object(&mut self, space: Space, name: &ObjectName, owner: Option<Key>) -> Key {
        let key = Key::of(space, name);
        self.create(key.clone(), owner, last(name).map(spellings));
        key
    }

    /// Creates the object `ident` names on its own, owned by `owner`.
    fn bare(&mut self, space: Space, ident: &Ident, owner: Option<Key>) {
        self.create(Key::bare(space, ident), owner, Some(spellings(ident)));
    }

    /// Creates the object `ident` of `owner`, which dies with it.
    fn member(&mut self, space: Space, owner: &Key, ident: &Ident) {
        let key = Key::under(space, owner, ident);
        self.create(key, Some(owner.clone()), Some(spellings(ident)));
    }

    fn create(&mut self, object: Key, owner: Option<Key>, names: Option<Vec<String>>) {
        self.out.changes.push(Change::Create {
            object,
            owner,
            names: names.unwrap_or_default(),
        });
    }

    /// Whether the statement being read runs when its document does, which no statement held
    /// by a body or by another statement does.
    fn runs(&self) -> bool {
        self.depth == 0 && self.enclosing == 0
    }

    /// Records a drop, which a statement that does not run never makes.
    fn remove(&mut self, change: Change) {
        if self.runs() {
            self.out.changes.push(change);
        }
    }

    /// Records a rename, which in a statement that does not run only declares the new name.
    fn rename(&mut self, from: Key, to: Key, owner: Option<Key>, names: Vec<String>) {
        let change = if self.runs() {
            Change::Rename {
                from,
                to,
                owner,
                names,
            }
        } else {
            Change::Create {
                object: to,
                owner,
                names,
            }
        };
        self.out.changes.push(change);
    }
}

/// The text of a string literal value, in each spelling PostgreSQL gives a body.
fn text(value: &Value) -> Option<&str> {
    match value {
        Value::DollarQuotedString(string) => Some(&string.value),
        Value::SingleQuotedString(string)
        | Value::EscapedStringLiteral(string)
        | Value::UnicodeStringLiteral(string) => Some(string),
        _ => None,
    }
}

/// The text of the body literal `body` and where it starts in the document.
fn source(body: &ValueWithSpan) -> Option<(&str, Location)> {
    text(&body.value).map(|text| (text, body.span.start))
}
