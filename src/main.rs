//! The `cargo-refquiem` binary: argument parsing and exit codes.

use std::io::Write as _;
use std::process::ExitCode;

use clap::Parser;

/// The largest step summary GitHub displays, in bytes.
const GITHUB_SUMMARY_LIMIT: usize = 1_048_576;

#[derive(clap::Parser)]
#[command(
    name = "refquiem",
    bin_name = "cargo refquiem",
    version,
    about = "Find mentions of Rust items that no longer exist",
    args_conflicts_with_subcommands = true
)]
struct Args {
    /// Scan this revision and the commits after it instead of the full history.
    #[arg(long, value_name = "REV", conflicts_with = "base")]
    since: Option<String>,
    /// Scan this revision plus the commits in `BASE..HEAD`.
    #[arg(long, value_name = "REV")]
    base: Option<String>,
    /// Compare against this revision instead of the working tree.
    #[arg(long, value_name = "REV", requires = "base")]
    head: Option<String>,
    /// Read this config file instead of the workspace's `.refquiem.toml`.
    #[arg(long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,
    /// Scan only the files of this workspace member for mentions.
    #[arg(long, value_name = "NAME")]
    package: Option<String>,
    #[command(flatten)]
    output: Output,
    /// Exit 0 even with findings.
    #[arg(long)]
    no_fail: bool,
    /// The `vocabularies` subcommand, which conflicts with every scan argument.
    #[command(subcommand)]
    command: Option<Command>,
}

/// An action instead of the default scan.
#[derive(clap::Subcommand)]
enum Command {
    /// Report the available or the workspace's effective vocabularies.
    Vocabularies(Vocabularies),
}

/// Vocabulary inspection arguments, which follow the `vocabularies` subcommand.
#[derive(clap::Args)]
struct Vocabularies {
    /// Report the workspace's effective selection instead of the catalog.
    #[arg(long)]
    effective: bool,
    /// Read this config file instead of the workspace's `.refquiem.toml`.
    #[arg(long, value_name = "PATH", requires = "effective")]
    config: Option<std::path::PathBuf>,
    /// Print the result as JSON.
    #[arg(long)]
    json: bool,
}

/// How the report is printed.
#[derive(Clone, Copy, clap::Args)]
struct Output {
    /// Print the report as JSON.
    #[arg(long)]
    json: bool,
    /// Print the report as Markdown.
    #[arg(long, conflicts_with_all = ["json", "github"])]
    markdown: bool,
    /// Add GitHub workflow annotations, and write the job summary when GitHub provides its file.
    #[arg(long)]
    github: bool,
}

impl Output {
    fn emit(self, report: &refquiem::Report, no_fail: bool) -> Result<(), refquiem::Error> {
        if self.json {
            println!("{}", report.json());
            return Ok(());
        }
        if self.markdown {
            let web_root = std::env::current_dir()
                .ok()
                .and_then(|root| web_root(&root));
            print!("{}", report.job_summary(web_root.as_deref()));
            return Ok(());
        }
        print!("{}", report.human());
        if self.github {
            print!("{}", report.annotations(no_fail));
            if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
                let path = std::path::PathBuf::from(path);
                let io = |source| refquiem::Error::Io {
                    path: path.clone(),
                    source,
                };
                let web_root = std::env::current_dir()
                    .ok()
                    .and_then(|root| web_root(&root));
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(io)?;
                std::io::Write::write_all(
                    &mut file,
                    report
                        .github_summary(web_root.as_deref(), GITHUB_SUMMARY_LIMIT)
                        .as_bytes(),
                )
                .map_err(io)?;
            }
        }
        Ok(())
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "refquiem") {
        args.remove(1);
    }
    let args = Args::parse_from(args);
    match &args.command {
        Some(Command::Vocabularies(vocabularies)) => match inspection(vocabularies) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("refquiem: {}", chain(&error));
                ExitCode::from(2)
            }
        },
        None => {
            let output = args.output;
            let no_fail = args.no_fail;
            match run(args).and_then(|report| output.emit(&report, no_fail).map(|()| report)) {
                Ok(report) if report.is_clean() || no_fail => ExitCode::SUCCESS,
                Ok(_) => ExitCode::FAILURE,
                Err(error) => {
                    eprintln!("refquiem: {}", chain(&error));
                    ExitCode::from(2)
                }
            }
        }
    }
}

/// A failed vocabulary inspection.
#[derive(Debug, thiserror::Error)]
enum InspectionError {
    /// The workspace discovery or the selected configuration failed.
    #[error(transparent)]
    Scan(#[from] refquiem::Error),
    /// The invocation directory could not be read or the output could not be written.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Vocabulary output could not be serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Report on the vocabularies instead of scanning.
fn inspection(vocabularies: &Vocabularies) -> Result<(), InspectionError> {
    if vocabularies.effective {
        let root = std::env::current_dir()?;
        let config = vocabularies.config.as_ref().map(|path| root.join(path));
        let mut options = refquiem::Options::new(root);
        options.config = config;
        let selection = refquiem::vocabulary::effective(&options)?;
        print_selection(&selection, vocabularies.json)
    } else {
        print_catalog(vocabularies.json)
    }
}

/// Print the catalog of the built-in vocabularies, without discovering a repository.
fn print_catalog(json: bool) -> Result<(), InspectionError> {
    let builtins = refquiem::vocabulary::available();
    let mut stdout = std::io::stdout().lock();
    if json {
        serde_json::to_writer(&mut stdout, builtins)?;
        writeln!(stdout)?;
        return Ok(());
    }
    print_builtins(&mut stdout, builtins)?;
    Ok(())
}

/// Print the workspace's effective vocabulary selection.
fn print_selection(
    selection: &refquiem::vocabulary::EffectiveVocabularies,
    json: bool,
) -> Result<(), InspectionError> {
    let mut stdout = std::io::stdout().lock();
    if json {
        serde_json::to_writer(&mut stdout, selection)?;
        writeln!(stdout)?;
        return Ok(());
    }
    writeln!(stdout, "Configuration {}", selection.config.display())?;
    writeln!(
        stdout,
        "Rust standard-library resolution {}",
        selection.rust_stdlib
    )?;
    if selection.builtins.is_empty() && selection.files.is_empty() {
        writeln!(stdout, "No optional vocabularies selected.")?;
    }
    print_builtins(&mut stdout, selection.builtins.iter().copied())?;
    print_files(&mut stdout, &selection.files)?;
    Ok(())
}

fn print_builtins<'a>(
    output: &mut impl std::io::Write,
    builtins: impl IntoIterator<Item = &'a refquiem::vocabulary::BuiltinVocabulary>,
) -> std::io::Result<()> {
    for builtin in builtins {
        writeln!(
            output,
            "{} ({}, {})",
            builtin.id, builtin.kind, builtin.case
        )?;
        writeln!(
            output,
            "  source {} @{}",
            builtin.source, builtin.source_version
        )?;
    }
    Ok(())
}

fn print_files(
    output: &mut impl std::io::Write,
    files: &[refquiem::vocabulary::CustomVocabulary],
) -> std::io::Result<()> {
    for file in files {
        writeln!(
            output,
            "{} ({}, {})",
            file.path.display(),
            file.kind,
            file.case
        )?;
        writeln!(output, "  format version {}", file.format_version)?;
    }
    Ok(())
}

/// The web root for the Markdown links. `GITHUB_SERVER_URL` and `GITHUB_REPOSITORY` when both
/// are set and non-empty, else the `origin` remote when it names a GitHub repository.
fn web_root(root: &std::path::Path) -> Option<String> {
    let repo = gix::discover(root).ok()?;
    let origin = repo.find_remote("origin").ok().and_then(|remote| {
        remote
            .url(gix::remote::Direction::Fetch)
            .map(std::string::ToString::to_string)
    });
    refquiem::web::root(
        std::env::var("GITHUB_SERVER_URL").ok().as_deref(),
        std::env::var("GITHUB_REPOSITORY").ok().as_deref(),
        origin.as_deref(),
    )
}

fn run(args: Args) -> Result<refquiem::Report, refquiem::Error> {
    let root = std::env::current_dir().map_err(|source| refquiem::Error::Io {
        path: ".".into(),
        source,
    })?;
    let config = args.config.map(|path| root.join(path));
    let mut options = refquiem::Options::new(root);
    options.range = match (args.since, args.base) {
        (Some(since), _) => refquiem::Range::Since(since),
        (None, Some(base)) => refquiem::Range::Base {
            base,
            head: args.head,
        },
        (None, None) => refquiem::Range::Full,
    };
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        options.cache_dir = Some(std::path::PathBuf::from(target).join("refquiem"));
    }
    options.config = config;
    options.package = args.package;
    refquiem::run(&options)
}

/// `error` followed by each of its causes.
fn chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}
