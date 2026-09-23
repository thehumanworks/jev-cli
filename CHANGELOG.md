# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Presets: a JSON file with the crate's questions, an ordered first-match rule table over the
  answers, and the actions the rules choose between. All the questions go in one request.
- `jev decide PRESET` prints the chosen action. `--json` prints `decision`, `rule` and `rules`
  before the usual response keys. It exits 1 when no rule matched and there is no fallback.
- `jev call PRESET` decides, then runs the chosen action. The action is an argv with no shell,
  gets the state on stdin, and gets `JEV_PRESET`, `JEV_ACTION` and `JEV_DECISION`. `call` exits
  with the action's status. `--json` collects the action's output under `result`.
- `--explain` on `decide` and `call` writes every rule, each condition's value and margin, the
  closest call, and the answers, to stderr.
- Presets can be named: `.jev/presets/NAME.json` from the working directory upwards, then
  `$XDG_CONFIG_HOME/jev/presets/NAME.json`.
- `jev example --preset` prints a preset over the example questions.
- A git preset in `.jev/presets/git.json`, so `jev call git -s "..."` takes a plain-language
  request. It covers status, diffs, history, staging, commits, stashes, branches, pull, push and
  worktrees. Names, paths and commit messages come from the request: a quoted string, or a
  branch, worktree or file that exists. It never forces, discards work or rewrites history.

### Changed

- Exit 1 also means that `decide` made no decision. Exit 2 covers a bad preset. Exit 7 covers an
  action that cannot be started. The help text for `--output` names the `decide` and `call`
  formats.
- Library: `Io::env` set to `Some` is now also the whole environment of an action that `call`
  runs, plus the `JEV_*` variables. `XDG_CONFIG_HOME` and `HOME` are read from it to find a named
  preset.

## [0.1.0] - 2026-09-22

### Added

- `jev ask`, `jev noul`, `jev choice`, `jev score`, `jev example`, and `jev completions`.
- Text and JSON output, `--dry-run`, `noul --threshold`, verbose reporting, and `--debug`
  lines written as each attempt fails.
- Exit codes 0–7 for success, a missed threshold, usage errors, authentication, rejected
  requests, context length, other API or network failures, and local I/O.
- A response body is not printed unless `--debug` is set, except the short serde fragment that
  names what was unreadable in an otherwise successful reply. The crate does not quote the request
  or the API key.
- `--timeout` and `JEV_TIMEOUT` must be from 1 to 86400 seconds.
- A questions document with a repeated id is rejected. A leading UTF-8 BOM on the state or the
  document is ignored.
- Release binaries for Linux (x86_64 and aarch64, statically linked with musl) and macOS (arm64
  and x86_64), attached to the GitHub release as `.tar.gz` archives with SHA-256 checksums.
