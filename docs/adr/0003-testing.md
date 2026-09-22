# ADR 0003: Testing strategy

- Status: Accepted
- Date: 2026-09-22

## Context

The client retries with real sleeps, reads the process environment, and posts to TypeSafe. A
test that did any of those would be slow, leak a key into a fixture, or spend money.

## Decision

`run` takes an `Io` value (arguments, stdin, stdout, stderr as an `Arc<Mutex<_>>` so `--debug`
can write during the call, whether stdin is a terminal, and an optional environment map) and an
optional `Transport`. Tests pass the map, so the process environment is never consulted. An
injected transport sets `backoff_scale` to 0.

Unit tests pin parsing, rendering and exit-code mapping. In-process tests drive every
subcommand. Binary tests use `assert_cmd`. Help output is compared to fixtures. HTTP status
mapping is a `std::net` server on `127.0.0.1:0` that answers one canned response. The live test
is `#[ignore]` and returns immediately when `TYPESAFE_API_KEY` is unset.

## Consequences

Nothing in `cargo test` opens a route except the loopback fake. The live test is opt-in because
it spends credits and sends a state to a third party. Help fixtures must be regenerated when
help text changes, and they are diffed by `scripts/check.sh`.
