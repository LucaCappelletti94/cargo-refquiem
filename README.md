# cargo-refquiem

[![CI](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/refquiem.svg)](https://crates.io/crates/refquiem)
[![docs.rs](https://docs.rs/refquiem/badge.svg)](https://docs.rs/refquiem)

A cargo requiem for the refs your Rust code left behind.

`cargo refquiem` walks your git history, finds every item, dependency, feature, binary and example that existed in some commit and is gone now, and reports the comments, doc comments, string literals and Markdown files that still name it, from paths, names written alone and rustdoc page links to `cargo add` and `-p` arguments, `--features` flags, `toml` snippets and serde spellings in `json` or `yaml` examples.

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

```yaml
on: [pull_request]

permissions:
  contents: read
  pull-requests: write

jobs:
  refquiem:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: LucaCappelletti94/cargo-refquiem@v1
        with:
          pr-number: ${{ github.event.pull_request.number }}
```

The action scans the commits the event brings, a pull request's own commits or a push's, fetching the history it needs from a shallow checkout. With `pr-number` it requests changes in a review with one comment per stale mention in a changed file, and without it the job fails on findings. Its earlier change requests on that pull request are dismissed once a run comes back clean or posts a newer review. A fork's token cannot post, so a fork pull request runs `mode: prepare` and a `workflow_run` job posts the payload with `mode: post`, as this repository's [dogfood workflows](https://github.com/LucaCappelletti94/cargo-refquiem/blob/main/.github/workflows/dogfood.yml) do. `dry-run: true` writes the review into the job summary, and a `workflow_dispatch` or scheduled run audits the whole history, from `since` when given.
