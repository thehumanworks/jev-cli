# jev

`jev` is a command-line client for [TypeSafe](https://typesafe.ai)'s Jev model, the System One
API. Jev does not generate text. You send a state (a piece of text, or a JSON object, array or
string) and one or more typed questions about it, and get back one calibrated answer per
question. A `noul` is a yes/no probability, a `choice` is one of a set of options with a
probability for each, and a `score` is a rating on an ordered scale of 2 to 10 levels.

Every question in a request is evaluated against the same state. The questions and answers are
the types from the [`typesafe-jev`](https://docs.rs/typesafe-jev) crate (0.2): this program does
not invent a second schema. Output is plain text for a person and a stable JSON document for a
script. There is no colour and no spinner.

## Install

Prebuilt binaries for Linux (x86_64, aarch64; static, musl) and macOS (arm64, x86_64) are
attached to each [GitHub release](https://github.com/thehumanworks/jev-cli/releases). Each
archive has a `.sha256` beside it.

```bash
target=aarch64-apple-darwin   # or x86_64-apple-darwin, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
version=v0.1.0
curl -fsSLO "https://github.com/thehumanworks/jev-cli/releases/download/$version/jev-$version-$target.tar.gz"
tar -xzf "jev-$version-$target.tar.gz"
install "jev-$version-$target/jev" ~/.local/bin/
```

From source:

```bash
cargo install --path .
```

The binary is `jev`. Set `TYPESAFE_API_KEY` in the environment. Passing the key as `--api-key`
works, but the environment variable is preferred: command-line flags are visible to other
processes.

## Quick start

```bash
jev noul "Does this message ask for a refund?" -s "I want my money back."

jev choice "Which team should handle this?" \
  billing="Payment or subscription issues" \
  technical="Bugs or integration problems" \
  sales="Pricing or account questions" \
  -s "The Stripe integration has failed for three days."

jev score "How frustrated the customer appears" \
  "Calm, just stating facts" \
  "Frustrated but civil" \
  "Very angry, strong language" \
  -s "This is the third time I have written. Nothing works."
```

Read the state from a file, or from a pipe (`-` is stdin for `--state-file`):

```bash
jev noul "Does this ask for a refund?" -f ticket.txt
jev noul "Does this ask for a refund?" -f - < ticket.txt
```

`--threshold` makes a yes/no question exit like `grep`. The probability is still printed. Exit 0
means it is at least `P`; exit 1 means it is below. Any other status is a real failure, so a
script that cares should look at the code:

```bash
jev noul "Is it spam?" -f mail.txt --threshold 0.8 && echo "at or above 0.8"

jev noul "Is it spam?" -f mail.txt --threshold 0.8
status=$?
if [ "$status" -eq 0 ]; then
  echo "spam"
elif [ "$status" -eq 1 ]; then
  echo "not spam"
else
  exit "$status"
fi
```

An option or a level that starts with `-` is written after `--`, and `--` comes after every flag:

```bash
jev choice "Which sign?" -s "the value fell" -- --down --up
```

## For scripts and agents

### `--json`

`--json` is shorthand for `--output json`. The two cannot be combined when they disagree
(`--json --output text` is a usage error; `--json --output json` is fine).

JSON output is one document for every command, pretty-printed, with a trailing newline. It is
the crate's `Response` (`model`, `answers`, `usage`) plus `cost_usd`. Key order is stable:
`model`, `answers`, `usage`, `cost_usd`. Answers keep the order the API returned and are tagged
with `"type": "noul" | "choice" | "score"`. `cost_usd` is `Usage::cost_usd` for that response's
input tokens, at the list price in the `typesafe-jev` crate. This shape is a contract.

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "q": {
      "type": "noul",
      "noul": 0.95
    }
  },
  "usage": {
    "input_tokens": 318,
    "output_tokens": 34
  },
  "cost_usd": 0.000013356
}
```

On a single-question command, text output is the bare value at full precision (Rust's default
decimal formatting), one line: the probability, the chosen option name, or the score. Use
`--json` when a script needs the probabilities, the confidence, or the cost. `ask` text output
is the multi-line form in the next section.

### `--dry-run`

`--dry-run` prints the JSON request body and exits 0. Nothing is sent, and no API key is
required. The body is what the client would POST, pretty-printed: `model`, `state`, `questions`,
in that order. Question order, choice-option order, and a noul's `criteria` keys `true` and
`false` are the crate's serde output. `--output`, `--verbose`, `--debug` and `--threshold` do
not change it. A threshold that is outside 0..=1 is still a usage error.

```bash
jev noul "Is it spam?" --yes spam --no "not spam" -s "You have won a prize" --dry-run
```

### Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success. For `noul --threshold`, the probability is at least P |
| 1 | `noul --threshold`: the probability is below P |
| 2 | Usage error: clap, a bad state, or a bad questions document |
| 3 | No API key, or the API rejected it |
| 4 | The API refused the request, or the configuration cannot be sent |
| 5 | The request exceeds the model's context |
| 6 | Any other API or network failure, including retries exhausted |
| 7 | Local I/O failure (unreadable file or stdin) |

Errors are one line on stderr, `jev: <message>`. A response body is not printed unless
`--debug` is given, except the short serde fragment that names what was unreadable in an
otherwise successful reply. The crate never quotes the request or the API key. A closed stdout (for
example `jev noul ... | head -1`) exits 0 and prints no error; in a pipeline that closes stdout
early the exit code is 0, not the threshold result. Success writes nothing to stderr unless `-v`
or `--debug` is set.

`-v` prints one line after a successful request, including when a threshold was missed:

```text
jev-1.13.0: 318 input tokens, 34 output tokens, 0 retries, ~$0.000013
```

The model name is the one the API reported. The cost is six decimal places. The word is always
`retries`. `--debug` prints each failed attempt on stderr as it happens, and keeps the response
body on the error line. Without it, an `HTTP <status>` body is omitted.

### Environment

A flag wins over the environment, and the environment wins over the default. An empty value is
ignored. `JEV_TIMEOUT` and `JEV_RETRIES` must be whole numbers. Repeating a flag uses the last
value.

| Variable | Flag | Default |
| --- | --- | --- |
| `TYPESAFE_API_KEY` | `--api-key` | none; required except for `--dry-run` |
| `JEV_BASE_URL` | `--base-url` | `https://api.typesafe.ai/v1/systemone` |
| `JEV_MODEL` | `--model` | `jev-latest` |
| `JEV_TIMEOUT` | `--timeout` | `60` seconds, from 1 to 86400. Also bounds connecting |
| `JEV_RETRIES` | `--retries` | `8` extra attempts after the first |

### Questions document

`jev ask` reads one JSON document, either `-q PATH` (`-` is stdin) or `--questions-json`. The
document is the crate's `Questions` value: an object keyed by question id, each value tagged
with `"type"`. A document that does not parse is a usage error, and the message includes serde's
description. An empty document is a usage error. A repeated id is a usage error naming that id.
A leading UTF-8 BOM on the state or the questions document is ignored. State and questions
cannot both be stdin.

`jev example` prints a document with one question of each type, built by serializing the crate's
types. `jev example --state` prints a state that fits those questions.

```bash
jev example | jev ask -q - -s "Please help ASAP" --dry-run
```

```json
{
  "department": {
    "type": "choice",
    "instructions": "Which team should handle this?",
    "criteria": {
      "billing": "Payment or subscription issues",
      "technical": "Bugs or integration problems",
      "sales": "Pricing or account questions"
    }
  },
  "frustration": {
    "type": "score",
    "instructions": "How frustrated the customer appears",
    "criteria": [
      "Calm, just stating facts",
      "Frustrated but civil",
      "Very angry, strong language"
    ]
  },
  "is_urgent": {
    "type": "noul",
    "instructions": "The message conveys urgency or time-sensitivity"
  }
}
```

The example state is: Hi, I've been trying to connect my Stripe account for 3 days and the integration keeps failing. I'm losing sales. Please help ASAP.

A choice option on the command line is `name` or `name=description`, split on the first `=`.
Names are unique and non-empty, and there are 2 to 255 of them. A score has 2 to 10 levels,
from the low end of the scale to the high end. The default question id is `q`.

`--state-json` parses the state as a JSON object, array or string and preserves object key
order. A number, boolean, null, or invalid JSON is a usage error. Text that is empty or only
whitespace is a usage error, including a JSON string of whitespace. With no `-s` and no `-f`,
the state is read from stdin. If stdin is a terminal, that is a usage error which names `-s`,
`-f`, and a pipe.

## Text output of `ask`

One block per question, in question order. Probabilities and scores use two decimal places.
Choice lines follow the order the API returned. Score levels run from low to high. A level
description that is JSON rather than text is printed as compact JSON.

```text
department: technical (confidence 0.78)
  billing: 0.15
  technical: 0.85
  sales: 0.00
frustration: 1.00 (confidence 0.70)
  0 Calm, just stating facts: 0.10
  1 Frustrated but civil: 0.80
  2 Very angry, strong language: 0.10
is_urgent: 0.95
```

## Development

```bash
scripts/check.sh    # fmt, clippy, tests, rustdoc, help fixtures
mise run check
mise run test       # cargo test; ignored live tests stay ignored
mise run lint
mise run fmt
```

Help text is pinned to `tests/fixtures/help/`. After changing a flag or its wording, rebuild and
regenerate, then read the diff:

```bash
cargo build --bin jev
bin=target/debug/jev
$bin --help > tests/fixtures/help/jev.txt
for c in ask noul choice score example completions; do
  $bin "$c" --help > "tests/fixtures/help/$c.txt"
done
```

`cargo test --test live -- --ignored` calls the real API when `TYPESAFE_API_KEY` is set. It
spends credits and sends the fixture state to TypeSafe. Run it only when you mean to. With the
variable unset, the test returns without calling the network.

No other test opens a socket except to `127.0.0.1`. `scripts/install-git-hooks.sh` points Git at
`scripts/pre-commit.sh` (fmt, clippy, tests).

## License

MIT OR Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.
