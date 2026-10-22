# cargo-refquiem

[![CI](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/refquiem.svg)](https://crates.io/crates/refquiem)
[![docs.rs](https://docs.rs/refquiem/badge.svg)](https://docs.rs/refquiem)

A cargo requiem for the refs your Rust code left behind.

`cargo refquiem` walks your git history, finds every item, dependency, feature, binary and example that existed in some commit and is gone now, and reports the comments, doc comments, string literals and Markdown files that still name it, from paths, names written alone and rustdoc page links to `cargo add` and `-p` arguments, `--features` flags, `toml` snippets and serde spellings in `json` or `yaml` examples. Parsed files and macro expansions are cached under `target/refquiem`, respecting `CARGO_TARGET_DIR` when set.

```sh
cargo refquiem                                   # every commit, against the working tree
cargo refquiem --since v0.3.0                    # from a release on
cargo refquiem --base origin/main --head HEAD    # what a branch removed
cargo refquiem --github --no-fail                # annotations and a job summary in GitHub Actions
cargo refquiem --json                            # the report as JSON
```

Macro-generated names, exported dependency and standard-library APIs, and plain English prose words count as alive, with implemented members governed by their `trait`'s public reachability. Removed dependency APIs use locked versions when available and require full crate-root paths for entirely dropped crates, with warnings for unavailable sources. Exit codes are `0` for no findings, `1` for findings and `2` for errors, including Markdown reader failures.

Each finding uses complete introduction history to name what the removed name most likely became, choosing among names introduced after it with `moved to`, `renamed to` or `replaced by` for a move, near spelling, matching signature or dependency-table replacement. Ambiguous candidates are listed without a suggestion. The `JSON` report carries rewritten mention lines, which the GitHub Action posts as suggested changes.

Release histories and generated files are not scanned. A `.refquiem.toml` at the workspace root adds excludes and names that are never reported, and a `refquiem: allow(Name)` comment silences one mention on its own line or the next. A bare `allow` name also keeps serialized spellings alive, and marker text in Markdown code has no suppression effect.

```toml
exclude = ["docs/migration/"]
allow = ["Parser::parse_expr", "structopt"]
since = "v0.3.0"
```

## GitHub Action

The [GitHub Action guide](https://github.com/LucaCappelletti94/cargo-refquiem/wiki/GitHub-Action) provides a source-install workflow for reviews and annotations, including separate trusted posting for pull requests from forks.

The [wiki](https://github.com/LucaCappelletti94/cargo-refquiem/wiki) covers a first finding, configuration, reports and integration. Coding agents can use the [`refquiem` skill](https://github.com/LucaCappelletti94/cargo-refquiem/blob/main/skills/refquiem/SKILL.md).
