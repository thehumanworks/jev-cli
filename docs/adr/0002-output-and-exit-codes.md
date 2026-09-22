# ADR 0002: Output contract and exit codes

- Status: Accepted
- Date: 2026-09-22

## Context

The same binary is used by a person at a terminal and by a shell or an agent. Those callers want
different amounts of detail, and they branch on the process status.

## Decision

`ask` text is one block per question, in question order, with two decimal places. Choice lines
follow the API's probability order. Score levels run from low to high; a JSON level description
is compact JSON. The single-question commands print one bare value at Rust's default float
formatting, so a script can capture it. `--json` is the crate's `Response` plus `cost_usd`,
pretty-printed, key order `model`, `answers`, `usage`, `cost_usd`.

Exit codes are 0 success (threshold met), 1 threshold missed, 2 usage, 3 authentication, 4
rejected request or unusable configuration, 5 context length, 6 any other API or network
failure, 7 local I/O. Errors are one `jev:` line on stderr. A response body is not printed
unless `--debug` is given, except the short serde fragment that names what was unreadable in an
otherwise successful reply. The crate never quotes the request or the key. `--debug` writes each
failed attempt as it happens and keeps the body on the error line. A broken pipe on stdout exits
0 with no error, including when a threshold would have been 1.

`--dry-run` prints the request body and does not send it. `-v` prints one usage line after a
successful request. The cost there is six decimal places; the JSON number is unrounded.
`--timeout` is from 1 to 86400 seconds.

## Consequences

Changing a format or an exit code is a contract change: README, changelog, and the tests that
pin the bytes. `--timeout` sets both the request timeout and the connect timeout, so one number
bounds the call.
