//! The names SQL declares, read with `sqlparser`, and the SQL it could not read.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, CreateFunction, CreateFunctionBody, DoOption, DoStatement, Expr, Ident,
    ObjectName, RenameTableNameKind, SchemaName, Statement, TableConstraint,
    UserDefinedTypeRepresentation, Value, ValueWithSpan, Visit, visit_statements,
};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Location, Tokenizer};
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
        file.rsplit('/').skip(1).find_map(|segment| {
            let named =
                |names: &[&str]| names.iter().any(|name| segment.eq_ignore_ascii_case(name));
            if named(&["postgres", "postgresql", "pg"]) {
                Some(Self::Postgres)
            } else if named(&["mysql", "mariadb"]) {
                Some(Self::Mysql)
            } else if named(&["sqlite"]) {
                Some(Self::Sqlite)
            } else {
                None
            }
        })
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
    let mut collector = Collector {
        dialect,
        depth: 0,
        body_at: None,
        out: Declared::default(),
    };
    match parse(source, dialect) {
        Ok(statements) => collector.statements(&statements),
        Err(refusal) => collector.out.refused.push(refusal),
    }
    collector.out
}

/// The statements of `source`, tokenized first so a tokenizer error keeps its location.
fn parse(source: &str, dialect: Dialect) -> Result<Vec<Statement>, Refusal> {
    let dialect = dialect.parser();
    let tokens = Tokenizer::new(dialect, source)
        .tokenize_with_location()
        .map_err(|error| Refusal {
            line: number(error.location.line),
            column: number(error.location.column),
            error: error.message,
        })?;
    Parser::new(dialect)
        .with_tokens_with_locations(tokens)
        .parse_statements()
        .map_err(|error| refusal(&error))
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

    fn constraint(&mut self, constraint: &TableConstraint) {
        let name = match constraint {
            TableConstraint::Unique(unique) => &unique.name,
            TableConstraint::PrimaryKey(key) => &key.name,
            TableConstraint::ForeignKey(key) => &key.name,
            TableConstraint::Check(check) => &check.name,
            TableConstraint::Index(index) => &index.name,
            TableConstraint::FulltextOrSpatial(index) => &index.opt_index_name,
            TableConstraint::PrimaryKeyUsingIndex(index)
            | TableConstraint::UniqueUsingIndex(index) => &index.name,
            TableConstraint::Exclude(exclude) => &exclude.name,
        };
        if let Some(name) = name {
            self.ident(name);
        }
    }

    fn function(&mut self, function: &CreateFunction) {
        self.object(&function.name);
        for arg in function.args.iter().flatten() {
            if let Some(name) = &arg.name {
                self.ident(name);
            }
        }
        let Some(
            CreateFunctionBody::AsBeforeOptions {
                body: Expr::Value(body),
                ..
            }
            | CreateFunctionBody::AsAfterOptions(Expr::Value(body)),
        ) = &function.function_body
        else {
            return;
        };
        let language = function
            .language
            .as_ref()
            .map(|language| language.value.as_str());
        self.body(language.unwrap_or("sql"), body);
    }

    fn do_block(&mut self, block: &DoStatement) {
        let mut language = "plpgsql";
        let mut code = None;
        for option in &block.options {
            match option {
                DoOption::Language(name) => language = &name.value,
                DoOption::LanguageLiteral(name) => {
                    if let Some(name) = text(&name.value) {
                        language = name;
                    }
                }
                DoOption::Code(body) => code = Some(body),
            }
        }
        if let Some(code) = code {
            self.body(language, code);
        }
    }

    /// Reads the body string `body` written in `language`, `plpgsql` through
    /// `sqlparser-plpgsql` and `sql` as statements of the document's dialect.
    fn body(&mut self, language: &str, body: &ValueWithSpan) {
        let Some(source) = text(&body.value) else {
            return;
        };
        let plpgsql = language.eq_ignore_ascii_case("plpgsql");
        if !plpgsql && !language.eq_ignore_ascii_case("sql") {
            return;
        }
        let at = *self.body_at.get_or_insert(body.span.start);
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

    /// The last part of `name`, which is the object a qualified name declares.
    fn object(&mut self, name: &ObjectName) {
        if let Some(ident) = name.0.last().and_then(|part| part.as_ident()) {
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

/// The text of a string literal value.
fn text(value: &Value) -> Option<&str> {
    match value {
        Value::DollarQuotedString(string) => Some(&string.value),
        Value::SingleQuotedString(string) => Some(string),
        _ => None,
    }
}
