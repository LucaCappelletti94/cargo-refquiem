//! The names SQL declares, read with `sqlparser`, and the SQL it could not read.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, CreateFunction, CreateFunctionBody, DoOption, DoStatement, Expr, Ident,
    ObjectName, ObjectNamePart, RenameTableNameKind, SchemaName, Statement, TableConstraint,
    UserDefinedTypeRepresentation, Value, ValueWithSpan, Visit, visit_statements,
};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Location, Token, TokenWithSpan, Tokenizer, Whitespace};
use sqlparser_plpgsql::embedded::{Block, Branch, Node};

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

/// The names a SQL document declares and the parts of it that could not be read.
#[derive(Debug, Default)]
pub(crate) struct Declared {
    pub(crate) names: Vec<String>,
    pub(crate) refused: Vec<Refusal>,
}

/// Everything `source`, read in `dialect`, declares, `DO` and function bodies included. A
/// document sqlparser refuses declares nothing, and a refused body loses only its own names.
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

/// Gathers the names of statements, reading the bodies they hold.
struct Collector {
    dialect: Dialect,
    /// How many bodies enclose the statements being read.
    depth: usize,
    /// Where the outermost body being read starts in the document, which every refusal inside
    /// it is reported at.
    body_at: Option<Location>,
    out: Declared,
}

impl Collector {
    /// Gathers the names of every statement in `tree`, nested ones included.
    fn statements<V: Visit>(&mut self, tree: &V) {
        let _ = visit_statements(tree, |statement| {
            self.statement(statement);
            ControlFlow::<()>::Continue(())
        });
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::CreateTable(table) => {
                self.object(&table.name);
                for column in &table.columns {
                    self.ident(&column.name);
                }
                for constraint in &table.constraints {
                    self.constraint(constraint);
                }
            }
            Statement::CreateVirtualTable { name, .. }
            | Statement::CreateSequence { name, .. }
            | Statement::CreateTrigger(sqlparser::ast::CreateTrigger { name, .. }) => {
                self.object(name);
            }
            Statement::CreateView(view) => {
                self.object(&view.name);
                for column in &view.columns {
                    self.ident(&column.name);
                }
            }
            Statement::CreateIndex(index) => {
                if let Some(name) = &index.name {
                    self.object(name);
                }
            }
            Statement::CreateRole(role) => {
                for name in &role.names {
                    self.object(name);
                }
            }
            Statement::CreateUser(user) => self.ident(&user.name),
            Statement::CreatePolicy(policy) => self.ident(&policy.name),
            Statement::CreateExtension(extension) => self.ident(&extension.name),
            Statement::CreateSchema { schema_name, .. } => match schema_name {
                SchemaName::Simple(name) | SchemaName::NamedAuthorization(name, _) => {
                    self.object(name);
                }
                SchemaName::UnnamedAuthorization(ident) => self.ident(ident),
            },
            Statement::CreateFunction(function) => self.function(function),
            Statement::CreateProcedure { name, params, .. } => {
                self.object(name);
                for param in params.iter().flatten() {
                    self.ident(&param.name);
                }
            }
            Statement::CreateDomain(domain) => {
                self.object(&domain.name);
                for constraint in &domain.constraints {
                    self.constraint(constraint);
                }
            }
            Statement::CreateType {
                name,
                representation,
            } => {
                self.object(name);
                match representation {
                    Some(UserDefinedTypeRepresentation::Composite { attributes }) => {
                        for attribute in attributes {
                            self.ident(&attribute.name);
                        }
                    }
                    Some(UserDefinedTypeRepresentation::Enum { labels }) => {
                        for label in labels {
                            self.ident(label);
                        }
                    }
                    _ => {}
                }
            }
            Statement::AlterTable(alter) => {
                for operation in &alter.operations {
                    self.alteration(operation);
                }
            }
            Statement::Do(block) => self.do_block(block),
            _ => {}
        }
    }

    fn alteration(&mut self, operation: &AlterTableOperation) {
        match operation {
            AlterTableOperation::AddColumn { column_def, .. } => self.ident(&column_def.name),
            AlterTableOperation::AddConstraint { constraint, .. } => self.constraint(constraint),
            AlterTableOperation::RenameColumn {
                new_column_name, ..
            } => self.ident(new_column_name),
            AlterTableOperation::ChangeColumn { new_name, .. }
            | AlterTableOperation::RenameConstraint { new_name, .. } => self.ident(new_name),
            AlterTableOperation::RenameTable {
                table_name: RenameTableNameKind::As(name) | RenameTableNameKind::To(name),
            } => self.object(name),
            _ => {}
        }
    }

    /// The constraint's name and, for MySQL's keys, the name of the index backing it.
    fn constraint(&mut self, constraint: &TableConstraint) {
        let (name, index) = match constraint {
            TableConstraint::Unique(unique) => (&unique.name, &unique.index_name),
            TableConstraint::PrimaryKey(key) => (&key.name, &key.index_name),
            TableConstraint::ForeignKey(key) => (&key.name, &key.index_name),
            TableConstraint::Check(check) => (&check.name, &None),
            TableConstraint::Index(index) => (&index.name, &None),
            TableConstraint::FulltextOrSpatial(index) => (&index.opt_index_name, &None),
            TableConstraint::PrimaryKeyUsingIndex(index)
            | TableConstraint::UniqueUsingIndex(index) => (&index.name, &None),
            TableConstraint::Exclude(exclude) => (&exclude.name, &None),
        };
        for ident in [name, index].into_iter().flatten() {
            self.ident(ident);
        }
    }

    fn function(&mut self, function: &CreateFunction) {
        self.object(&function.name);
        for arg in function.args.iter().flatten() {
            if let Some(name) = &arg.name {
                self.ident(name);
            }
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

    /// The last part of `name`, which is the object a qualified name declares. Only
    /// Snowflake's `IDENTIFIER(…)` ends a name in something else, which no dialect read here
    /// parses.
    fn object(&mut self, name: &ObjectName) {
        for ident in name
            .0
            .last()
            .into_iter()
            .filter_map(ObjectNamePart::as_ident)
        {
            self.ident(ident);
        }
    }

    /// `ident` as written, and lowercased as PostgreSQL folds it when unquoted.
    fn ident(&mut self, ident: &Ident) {
        if ident.quote_style.is_none() {
            let folded = ident.value.to_lowercase();
            if folded != ident.value {
                self.out.names.push(folded);
            }
        }
        self.out.names.push(ident.value.clone());
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
