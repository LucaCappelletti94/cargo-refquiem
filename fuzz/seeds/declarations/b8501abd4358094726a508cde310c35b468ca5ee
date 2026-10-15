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
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "refquiem") {
        args.remove(1);
    }
    let args = Args::parse_from(args);
    match run(args) {
        Ok(report) => {
            print!("{}", report.human());
            if report.is_clean() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("refquiem: {}", chain(&error));
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> Result<refquiem::Report, refquiem::Error> {
    let root = std::env::current_dir().map_err(|source| refquiem::Error::Io {
        path: ".".into(),
        source,
    })?;
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
