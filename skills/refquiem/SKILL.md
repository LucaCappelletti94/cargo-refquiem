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

Running text whose clause states the removal, rename or replacement is no finding, as in `Delete X and Y`, `X is deleted`, `the removal of X`, `with X gone`, `X does not exist`, `Rename X to Y`, `X was renamed Y`, `Y replaces X`, `X is superseded by Y` or `in place of X`. `X becomes Y` counts only when `Y` is a code span or a code-shaped name. A sentence that describes the name as current is reported, and so are links and code blocks naming it, whatever the sentence says.

A suggestion names where the item most likely went. Verify it before applying.

- `moved to`, `renamed to`, `replaced by` — open the suggested path, check it holds the same kind and meaning, then point the references at it.
- `perhaps A or B` — several candidates and no winner, so decide from the removal commit.
- no suggestion — find what the item became with `git log -S` on the removal commit.

Fix the references themselves. Matching uses parsed names rather than type checking, so run the affected build or documentation checks after rewriting them.

## Changed items

A `--base` scan also reports, under `changed` in JSON, each item whose definition the range changed while a comment, doc comment or Markdown file the range left as it was still names it. The shape counts by default (signatures, fields, variants, trait items, alias targets, `serde` and `repr` attributes), and so do `const` and `static` values. Whitespace, comments, visibility and other attributes never count. A bare name counts when it names exactly one item, or when exactly one of the items carrying it is public or belongs to the mentioning file's crate. Reread each mention against the new definition and rewrite what no longer holds.

```toml
[changed]
level = "warn"     # "deny" fails the run, "allow" skips the check
compare = "values" # "shapes", or "bodies" to add function bodies
```

## Skipped dependencies

`skipped` entries identify removed dependencies whose sources Cargo could not resolve. Their items were not checked, and these warnings do not change the exit status or clean verdict. Read each reason before claiming coverage of historical dependency APIs.

## SQL names

The tables, columns, roles, functions and other objects head's SQL declares, `DO` and PL/pgSQL function bodies included, stay alive, so a removed Rust item sharing a live SQL object's name is not reported. That SQL is every `.sql` file, every ```` ```sql ```` fence in Markdown and doc comments, and every Rust string literal opening with `CREATE`, `ALTER`, `DROP` or `DO`. A file is read in the dialect its nearest `postgres`, `mysql`, `mariadb` or `sqlite` directory names, else in sqlparser's generic dialect, and a `[sql-dialects]` table maps globs to dialects ahead of both. A fence written ```` ```postgres ````, ```` ```mysql ```` or ```` ```sqlite ```` takes that dialect. `refused_sql` entries locate SQL that could not be read, whose names are therefore not known. These warnings do not change the exit status. An `exclude` glob drops a file that is not SQL, such as a `format!` template.

```toml
[sql-dialects]
"db/**/*.sql" = "postgres" # or "mysql", "sqlite", "generic"
```

## Suppress

- `refquiem: allow(name)` on a mention's own line or the line above it hides that mention.
- `allow = [...]` in `.refquiem.toml` hides a group by its path, and a bare name also drops the mentions of that identifier.
- `exclude = [...]` skips files before they are read. Generated files and commented-out code are skipped by default, and `scan-generated` and `scan-commented-code` override that.

Write a local marker in a Rust line comment or Markdown HTML comment. Marker text inside Markdown code has no suppression effect.

A marker that hides nothing becomes a finding, `refquiem: allow(name) suppresses nothing`. The range never changes that verdict. A `--since` or `--base` scan keeps a marker the full history scan needs, even when the removal it covers lies before the range.

## Vocabularies

English is the default optional vocabulary. An explicit `[vocabularies]` table replaces the optional selection for every scanned workspace file.

```toml
[vocabularies]
builtins = ["english", "sql@2023", "posix@2017"]
files = ["vocab/domain.toml"]
```

An empty table disables optional vocabularies. Omitted arrays default to empty, and selecting only `files` never enables `english`. The built-in identities are `english`, `sql@2023`, `postgres@18`, `sqlite@3.54`, `wasi@preview1`, `wasm@3.0`, `android@36`, `uikit@27.1`, `posix@2017`, `web-api@2026-10`, `python@3.15`, `javascript@2026-10` and `svg@2026-10`, and unknown identities or fields are configuration errors. `--config PATH` selects a replacement configuration, while `--package NAME` only limits scanned files.

`sql@2023` holds the reserved key words of SQL:2023 parts 2, 9 and 14 as a technical vocabulary matched case-sensitively, so `ARRAY_AGG` is ambiguous while `array_agg` stays reportable. Write a SQL key word in uppercase for it to count as SQL. A reported bare name whose uppercase spelling a selected exact-case vocabulary holds carries that spelling and vocabulary, as `uppercase` in JSON and as a `write ARRAY_AGG to mark it as sql@2023` note elsewhere.

`postgres@18` holds the functions the PostgreSQL 18 catalog defines, type input and output functions such as `interval_in` included, with the key-word functions its documentation lists, its configuration parameters in lowercase, such as `search_path` and `datestyle`, and its catalog tables and views, such as `pg_class` and `pg_roles`. `sqlite@3.54` holds the functions a SQLite 3.54 build registers. Both hold lowercase names matched case-sensitively, such as `set_config`, `uuidv7`, `json_extract` and `strftime`, and keep every name with an inner underscore and the single words English lacks, so `date`, `text`, `timezone` and `coalesce` stay reportable.

`wasi@preview1` holds the 46 WASI Preview 1 function names, such as `fd_write` and `path_open`, and `wasm@3.0` the WebAssembly 3.0 instructions whose text-format name has no dot, such as `call_indirect` and `br_table`, both as technical vocabularies matched case-sensitively. Dotted names such as `memory.grow` never count as a mention of a Rust name.

`android@36` holds the compound class names and camelCase method names of the public Android 16 framework API, such as `NativeActivity` and `onCreate`, as a technical vocabulary matched case-sensitively. Single-word classes such as `Activity`, single-word methods such as `close` and constants such as `ACTION_VIEW` stay reportable.

`uikit@27.1` holds the compound Objective-C class and protocol names and the camelCase selector heads of UIKit in the Xcode 27.1 SDK, such as `UIViewController`, `viewDidLoad` and `isExclusiveTouch`, as a technical vocabulary matched case-sensitively. Of a selector such as `tableView:cellForRowAtIndexPath:` only the head `tableView` can count as a mention. `Info.plist` keys such as `NSBluetoothAlwaysUsageDescription` and single words such as `close` stay reportable.

`posix@2017` holds POSIX.1-2017 names from the POSIX manual pages, the header constants and functions with an inner underscore such as `O_RDONLY`, `PATH_MAX` and `pthread_create`, the error numbers of `errno.h` such as `EINVAL` and the signals of `signal.h` such as `SIGTERM`, as a technical vocabulary matched case-sensitively. Single-word functions and constants such as `strlen`, `close` and `BUFSIZ` stay reportable.

`web-api@2026-10` holds the standards-track Web API interface and member names of MDN's browser compatibility data as of October 2026 that are compound or camelCase, such as `HTMLElement`, `CanvasRenderingContext2D` and `querySelector`, as a technical vocabulary matched case-sensitively. Single words such as `Document` and `fetch` stay reportable, and dotted spellings such as `document.getElementById` never count as a mention.

`python@3.15` holds the Python 3.15 built-in and standard-library object names of the CPython documentation that are compound or camelCase, such as `ValueError`, `OrderedDict` and `addHandler`, as a technical vocabulary matched case-sensitively. Snake names such as `read_text`, uppercase constants such as `AF_UNIX` and single words such as `len` stay reportable.

`javascript@2026-10` holds the mixed-case JavaScript built-in globals and members of the same data, such as `ArrayBuffer`, `Uint8Array` and `hasOwnProperty`, and `svg@2026-10` the camelCase SVG element and attribute names, such as `linearGradient` and `viewBox`, both as technical vocabularies matched case-sensitively. Constants such as `PI` and `MAX_SAFE_INTEGER` and single words such as `Promise` stay reportable.

A custom file is a self-describing `TOML` document with `format-version = 1`, `kind` as `natural-language` or `technical`, `case` as `sensitive`, `ascii-insensitive` or `unicode-lowercase`, and `words` as a string array. Missing fields, unknown fields, and unsupported values are errors.

```toml
format-version = 1
kind = "technical"
case = "sensitive"
words = ["PrivateSDKThing", "memory.grow"]
```

Relative paths resolve from the directory of the selected configuration, absolute paths select local files, canonical aliases select one file, and edits take effect on the next run. Missing, unreadable, non-UTF-8, malformed, or invalid files abort the run with the offending path. Words are nonempty without whitespace or control characters, and duplicates after the declared case normalization count once. Matching never trims words, strips punctuation, or applies Unicode composition normalization or full case folding.

Natural-language vocabularies establish ambiguity only in running text, including plain diagnostic text. English uses Unicode lowercasing. Technical vocabularies also establish ambiguity in inline code spans. Rust-specific links, qualified Rust paths, compiled Rust identifiers, serialized names, and manifest entries receive no vocabulary exemption, and matching uses the complete spelling under each vocabulary's case rule. Rust standard-library resolution is always active.

`cargo refquiem vocabularies` lists bundled identities, matching rules, and source revisions, including outside a repository, and never reads project configuration. `cargo refquiem vocabularies --effective` resolves the workspace selection without scanning history, lists selected built-ins and resolved custom files with their category, case rule, and format version, and accepts `--config PATH` to select another configuration. Inspection flags follow the subcommand, both modes support `--json`, and their exit status is `0` on success or `2` on error.

## Rerun

Rerun the same command until it exits `0`.
