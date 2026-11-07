# moose-spec-lint

Lint [MOOSE](https://mooseframework.inl.gov) test specification (`tests`) files for
SQA metadata, misspelled parameters, dangling file references, and structural
mistakes — in milliseconds, without booting the Python TestHarness.

## Why

MOOSE's SQA process requires every test to carry `requirement`, `design`, and
`issues` metadata, and the TestHarness silently ignores misspelled parameters.
Those failures surface late (SQA audits) or never (a `requirment = '...'` typo
means the requirement goes unrecorded). This linter catches them statically.

## Checks

| Check | Severity |
|---|---|
| HIT syntax errors | error |
| Unknown tester type | error (bypass with `--allow-tester NAME`) |
| Unknown parameter for the tester type, with "did you mean" hints | warning |
| Group section parameter outside the base Tester set | warning |
| RunApp-derived test with neither `input` nor `command` | warning |
| Diff tester (CSVDiff/Exodiff/JSONDiff/XMLDiff/ImageDiff) missing its file list parameter | error |
| Missing or empty `requirement` / `design` / `issues` (honoring group inheritance) | error (`--no-sqa` to disable) |
| Referenced `input` file does not exist | error |
| Referenced golden file does not exist (checks both the spec directory and `gold/`, honoring `gold_dir`) | error |
| `prereq` naming a test not present in the same file | warning |
| Duplicate test path in one file | warning |
| Parameterized section with no `type` and no children | warning |

Tester parameter tables are transcribed from `python/TestHarness/testers/` in the
MOOSE repository (framework testers plus their inheritance chains).

## Install

```sh
cargo install --git https://github.com/andrewwhitecdw/moose-spec-lint
```

## Usage

```sh
# Lint every `tests` file under a directory tree
moose-spec-lint test/tests modules/fluid_properties/test

# Single file, CI annotations for GitHub Actions
moose-spec-lint --format github modules/contact/test/tests

# Application testers, no SQA requirements, warnings fail the build
moose-spec-lint --allow-tester BisonTest --allow-tester ExtraDiff --no-sqa --strict .
```

Exit status is `1` when any error is found (or any warning with `--strict`), so
the tool drops straight into `pre-commit` hooks or CI jobs.

## Scope

The HIT parser covers the constructs used in test-spec files: nested sections
(`[./name]`, `[../sibling]`, `[]`), bare/quoted/`'''` values, and comments.
Substitutions, `!include`, and indexed parameter arrays are reported as parse
errors rather than silently misparsed.

## Relationship to MOOSE

MOOSE is developed by Idaho National Laboratory and licensed LGPL-2.1. This tool
is an independent MIT-licensed project that reads MOOSE spec files; it does not
embed MOOSE code.

## License

MIT
