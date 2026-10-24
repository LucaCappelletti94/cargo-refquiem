//! The `cargo-refquiem` binary: argument parsing and exit codes.

use std::process::ExitCode;

use clap::Parser;

#[derive(clap::Parser)]
#[command(
    name = "refquiem",
    bin_name = "cargo refquiem",
    version,
    about = "Find mentions of Rust items that no longer exist"
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
    fn emit(
        self,
        report: &refquiem::Report,
        no_fail: bool,
        web_root: Option<&str>,
    ) -> Result<(), refquiem::Error> {
        if self.json {
            println!("{}", report.json());
            return Ok(());
        }
        if self.markdown {
            print!("{}", report.job_summary(web_root));
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
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(io)?;
                std::io::Write::write_all(&mut file, report.job_summary(web_root).as_bytes())
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
    let output = args.output;
    let no_fail = args.no_fail;
    let web_root = std::env::current_dir()
        .ok()
        .and_then(|root| web_root(&root));
    match run(args).and_then(|report| {
        output
            .emit(&report, no_fail, web_root.as_deref())
            .map(|()| report)
    }) {
        Ok(report) if report.is_clean() || no_fail => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("refquiem: {}", chain(&error));
            ExitCode::from(2)
        }
    }
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
