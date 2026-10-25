---
name: refquiem
description: Run cargo refquiem and fix mentions of deleted Rust items, Cargo entries and dependency items in comments, doc comments, Markdown and string literals. Use when a refquiem check, review or CI run flags a stale mention, or when the user asks to find or fix old references.
license: MIT
---

# refquiem

`cargo refquiem` finds names deleted in the commit history that the repository still mentions in comments, doc comments, Markdown and string literals. Exit `1` on findings or unused markers, `2` on error.

## Range

The default is the full history, compared against the working tree.

- `--since REV` scans `REV` and the commits after it, up to `HEAD`.
- `--base REV` scans `REV` plus the commits of `REV..HEAD`.
- `--base BASE --head HEAD` indexes `BASE` plus the commits of `BASE..HEAD` and reads mentions from the specified head commit.

`--head` requires `--base`. `--since` conflicts with `--base`. A revision that does not resolve to a commit is an error.

## Run

```
cargo refquiem                     # full history, working tree
cargo refquiem --since HEAD~50     # the commits since fifty commits ago
cargo refquiem --base main --head feature
cargo refquiem --json              # the report as one JSON object
cargo refquiem --github --no-fail  # CI annotations at warning level
cargo refquiem --markdown          # a table with each finding's status and suggested action, linked when the repository is on GitHub
```

`--config PATH` reads a different config file. `--package NAME` reads mentions only from the files of one workspace member.

## Findings

Each group shows the removed name, its kind, the removal commit id or `working tree`, and the name it most likely became. Mentions list `file:line:column` with the context, like `doc comment code block` or `Markdown code span`.

A suggestion names where the item most likely went. Verify it before applying.

- `moved to`, `renamed to`, `replaced by` — open the suggested path, check it holds the same kind and meaning, then point the references at it.
- `perhaps A or B` — several candidates and no winner, so decide from the removal commit.
- no suggestion — find what the item became with `git log -S` on the removal commit.

Fix the references themselves. Matching uses parsed names rather than type checking, so run the affected build or documentation checks after rewriting them.

## Skipped dependencies

`skipped` entries identify removed dependencies whose sources Cargo could not resolve. Their items were not checked, and these warnings do not change the exit status or clean verdict. Read each reason before claiming coverage of historical dependency APIs.

## Suppress

- `refquiem: allow(name)` on a mention's own line or the line above it hides that mention.
- `allow = [...]` in `.refquiem.toml` hides a group by its path, and a bare name also drops the mentions of that identifier.
- `exclude = [...]` skips files before they are read. Generated files and commented-out code are skipped by default, and `scan-generated` and `scan-commented-code` override that.

Write a local marker in a Rust line comment or Markdown HTML comment. Marker text inside Markdown code has no suppression effect.

A marker that hides nothing becomes a finding, `refquiem: allow(name) suppresses nothing`.

## Vocabularies

English is the default optional vocabulary. An explicit `[vocabularies]` table replaces the optional selection for every scanned workspace file.

```toml
[vocabularies]
builtins = ["english"]
```

An empty table or `builtins = []` disables optional vocabularies. `english` is the available built-in identity, and unknown identities or fields are configuration errors. `--config PATH` selects a replacement configuration, while `--package NAME` only limits scanned files.

English uses Unicode lowercasing and establishes ambiguity only in running text, including plain diagnostic text. Inline code spans, Rust-specific links, qualified Rust paths, compiled Rust identifiers, and serialized names receive no English exemption. Rust standard-library resolution is always active.

## Rerun

Rerun the same command until it exits `0`.
