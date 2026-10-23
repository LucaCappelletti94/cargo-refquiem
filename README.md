# cargo-refquiem

[![CI](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml/badge.svg)](https://github.com/LucaCappelletti94/cargo-refquiem/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/LucaCappelletti94/cargo-refquiem/graph/badge.svg)](https://codecov.io/github/LucaCappelletti94/cargo-refquiem)
[![crates.io](https://img.shields.io/crates/v/refquiem.svg)](https://crates.io/crates/refquiem)
[![docs.rs](https://docs.rs/refquiem/badge.svg)](https://docs.rs/refquiem)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/LucaCappelletti94/cargo-refquiem/blob/main/LICENSE)

A cargo requiem for the refs your Rust code left behind.

`cargo refquiem` finds the Rust items, dependencies, features, binaries and examples your git history removed, and reports the comments, doc comments, string literals and Markdown that still name them, with what each most likely became. The [wiki](https://github.com/LucaCappelletti94/cargo-refquiem/wiki) explains matching, configuration, reports and the Action in depth.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/LucaCappelletti94/cargo-refquiem/main/docs/review-dark.svg">
  <img alt="A refquiem pull request review flags a stale Rust function name and suggests its replacement" src="https://raw.githubusercontent.com/LucaCappelletti94/cargo-refquiem/main/docs/review-light.svg">
</picture>

## Usage

```sh
cargo install refquiem --locked
cargo refquiem                                 # every commit, against the working tree
cargo refquiem --base origin/main --head HEAD  # what a branch removed
cargo refquiem --json                          # the report as JSON
cargo refquiem --markdown > report.md         # findings with suggested actions
```

The exit status is `0` when nothing is stale, `1` with findings and `2` on errors. A `.refquiem.toml` at the workspace root adds excludes and names never reported, a `refquiem: allow(Name)` comment silences one mention, and `cargo refquiem --help` lists every option.

## GitHub Action

The [action guide](https://github.com/LucaCappelletti94/cargo-refquiem/wiki/GitHub-Action) reviews each pull request with a suggested fix for every stale mention, pull requests from forks included.

## Library

Depend on `refquiem` with `default-features = false` to drop the CLI, then call `refquiem::run` as the [Library page](https://github.com/LucaCappelletti94/cargo-refquiem/wiki/Library) shows. Coding agents get the same guidance from the [`refquiem` skill](https://github.com/LucaCappelletti94/cargo-refquiem/blob/main/skills/refquiem/SKILL.md).

```sh
npx skills add LucaCappelletti94/cargo-refquiem
```
