# Architecture decisions

| ADR | Status | Decision |
| --- | --- | --- |
| [0001](0001-cli-shape.md) | Accepted | One subcommand per question type, plus the crate's JSON document for a batch |
| [0002](0002-output-and-exit-codes.md) | Accepted | Text, bare values, and JSON are a contract; exit codes are 0–7 |
| [0003](0003-testing.md) | Accepted | Injected IO and transport, a fake HTTP server, pinned help, no network |
| [0004](0004-lint-policy.md) | Accepted | One lint table in `Cargo.toml`; `-D warnings` in every gate |
| [0005](0005-library-and-binary.md) | Accepted | A library `run` and a binary that only wires process IO |
| [0006](0006-presets.md) | Accepted | Presets: the crate's questions, a first-match rule table, and actions; `decide` and `call` |

0001–0005 are dated 2026-09-22. 0006 is dated 2026-09-23.
