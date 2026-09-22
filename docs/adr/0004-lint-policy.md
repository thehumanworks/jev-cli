# ADR 0004: Lint policy

- Status: Accepted
- Date: 2026-09-22

## Context

Clippy configuration that lives in a script drifts from the configuration a person runs by hand.
This crate is small, and its output and errors are part of a contract, so a panic or a stray
print is a bug rather than a style choice.

## Decision

All levels are in `Cargo.toml` `[lints]`. `unsafe_code` is forbidden. `missing_docs` and
`unreachable_pub` warn. Clippy `all` and `pedantic` warn at priority -1, so the explicit denies
win: `correctness`, `suspicious`, `unwrap_used`, `expect_used`, `panic`, `dbg_macro`, `todo`,
`unimplemented`, `print_stdout`, `print_stderr`, and the same bug-class deny list as
`typesafe-jev` (lossy floats, string slices, wild error arms, and the rest of that list).

`scripts/check.sh`, `scripts/pre-commit.sh` and CI pass `-D warnings`, so every warn fails.
Test modules may allow `unwrap_used`, `expect_used` and `panic` at the top of the module only.
Any other allow needs a one-line reason on the attribute.

`rustfmt.toml` sets `max_width = 120` and `use_small_heuristics = "Max"`. The toolchain file
pins Rust 1.98.1. `rust-version` in the manifest is `1.98`.

## Consequences

A new lint from a compiler upgrade fails the gate until the code is fixed or the allow is
justified. Output goes through the writers `run` is given, which is what makes the in-process
tests possible.
