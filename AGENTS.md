# Instructions for coding agents

`jev` is a single Rust binary over the `typesafe-jev` crate. The README is the reference for
behaviour, flags and output. `docs/adr/` records why. This file is what an agent needs that
those do not say.

## Quality gate

`scripts/check.sh` is the gate CI runs: `cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test`, `cargo doc --no-deps` with `RUSTDOCFLAGS="-D
warnings"`, then a diff of every command's `--help` against `tests/fixtures/help/`. `mise run
check` is the same script. Lint levels live only in `Cargo.toml` `[lints]` (ADR 0004). A `warn`
fails CI. Do not add a lint allow except the three test allows named below, or one allow with a
one-line reason next to it.

`scripts/pre-commit.sh` is fmt, clippy and tests. Install it with `scripts/install-git-hooks.sh`.
Skip one commit with `JEV_SKIP_HOOKS=1` or `SKIP=1`.

## Contracts

Output formats and exit codes are a public contract covered by tests. Changing them means
updating the README, `CHANGELOG.md`, and any help fixture in the same change.

`tests/fixtures/help/<command>.txt` is the exact `--help` output (`jev.txt` is the root).
Regenerate after a help change:

```bash
cargo build --bin jev
bin=target/debug/jev
$bin --help > tests/fixtures/help/jev.txt
for c in ask noul choice score decide call example completions; do
  $bin "$c" --help > "tests/fixtures/help/$c.txt"
done
```

Read the diff. Help is rendered at a fixed width of 100 columns, so `COLUMNS` must not change it.

The questions document is the crate's `Questions` serde format. Do not re-model it. Build
examples with the crate's constructors and serialize them, which is what `jev example` does.
Environment variables are resolved in `cli.rs` after parsing, never via clap `env` attributes,
so help text stays free of values. `Io::stderr` is an `Arc<Mutex<_>>` (`Write + Send + 'static`)
so `--debug` can write each attempt as it happens; do not replace it with a bare writer.

## Tests

No test touches the network. In-process tests call `jev::run` with an injected
`typesafe_jev::Transport` and an `Io.env` map, so they never see the developer's
`TYPESAFE_API_KEY`. An injected transport sets `backoff_scale` to 0. Binary tests that need HTTP
use the fake server in `tests/common/mod.rs` on `127.0.0.1:0`.

`tests/git_preset.rs` runs `.jev/presets/git/run.sh` against temporary repositories, so it needs
`git` and `bash` on `PATH` and is unix-only. Its remote is a bare repository beside the test
repository, and it reads no git configuration of the developer's.

`tests/live.rs` is `#[ignore]`. It spends credits and sends a short state to TypeSafe. Run
`cargo test --test live -- --ignored` only when asked. If `TYPESAFE_API_KEY` is unset it returns
without calling the API.

Test modules may `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` at the top
of the module or integration-test file only.

## CI runners

CI runs on Modal when the `CI_RUNNER` repository variable is `modal` (`ci/modal_runner.py`,
`ci/setup-modal-runner.sh`, `docs/ci-runners.md`), and on GitHub-hosted runners otherwise. The
Rust version baked into the Modal image must match `rust-toolchain.toml`; redeploy with
`modal deploy ci/modal_runner.py` after changing it.

## Releases

CI builds release archives for four targets on every run (`scripts/package.sh`, uploaded as
workflow artifacts). To publish: set `version` in `Cargo.toml`, give that version a dated section
in `CHANGELOG.md`, commit, then push a tag `v<version>`. The `release` job fails if the tag and
`Cargo.toml` disagree, and uses the CHANGELOG section as the release notes. Tag only when asked.

## Commits

Commit only when asked. Do not commit `mise.local.toml` or a secret.
