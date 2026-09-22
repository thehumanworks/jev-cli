# ADR 0001: CLI shape

- Status: Accepted
- Date: 2026-09-22

## Context

A request is one state and a set of typed questions. The three question types have different
fields: a noul has optional yes/no meanings, a choice has named options, a score has ordered
levels. A batch is already a JSON object in `typesafe-jev`, keyed by id and tagged with `type`.

## Decision

`ask` reads that document and does not grow a parallel flag grammar. `noul`, `choice` and
`score` each ask one question, with the varying parts as positional arguments (`name` or
`name=description`, levels from low to high). `example` prints a document built by serializing
the crate's own constructors. `completions` prints a shell script.

Inline multi-question flags were rejected. They would re-model `Questions`, drift from the
crate, and make a batch of mixed types unreadable.

## Consequences

The document format is serde's format for `Questions`. A parse error includes serde's message.
Single-question commands validate counts locally (2..=255 options, 2..=10 levels) before the
request is sent. A document is sent as parsed; the API still enforces its own limits.
