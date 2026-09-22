# ADR 0005: Library plus a thin binary

- Status: Accepted
- Date: 2026-09-22

## Context

Exit codes, help text and rendered answers need tests that do not spawn a process for every
case, and that can supply a fake HTTP response. A binary that parses arguments and calls the
client directly cannot be driven that way without rewriting it.

## Decision

`src/main.rs` builds an `Io` from the real stdin, stdout, stderr and `IsTerminal`, then calls
`jev::run` and returns the status as `ExitCode`. Everything else is a library module with one
job: `cli` (clap definitions), `state`, `questions`, `render`, `exit`, `run`. Failures are a
typed enum that carries the exit code. `anyhow` is not a dependency.

The binary leaves the crate's user agent and provider as they are. It sets the model, base URL,
timeout, connect timeout and retry count from the flags.

`Io::stderr` is an `Arc<Mutex<E>>` with `E: Write + Send + 'static`, not a bare writer.
`Client::set_debug_reporter` stores a `'static + Send + Sync` closure, and `--debug` has to
print each failed attempt while `ask` is still blocked on the network. The reporter clones the
`Arc` and writes under the mutex as each attempt fails. `run` keeps that same mutex for the
error line and the verbose line afterwards. A `StderrLock`, or a writer captured by value,
cannot satisfy the closure's lifetime, and holding the lines until `ask` returns would hide
them for the whole retry.

## Consequences

New behaviour is tested by calling `run`. The public library surface is `run` and `Io`. Adding
a field to `Io` is a breaking change for embeddings; the binary is the product.
