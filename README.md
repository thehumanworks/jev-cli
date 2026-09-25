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

To choose and run a script for a state, see [Presets](#presets-decide-and-call).

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
input tokens, at the list price in the `typesafe-jev` crate. This shape is a contract. `decide`
and `call` put their decision before these keys; see [Presets](#presets-decide-and-call).

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
| 1 | `noul --threshold`: the probability is below P. `decide`: no rule matched and there is no fallback |
| 2 | Usage error: clap, a bad state, a bad questions document, or a bad preset |
| 3 | No API key, or the API rejected it |
| 4 | The API refused the request, or the configuration cannot be sent |
| 5 | The request exceeds the model's context |
| 6 | Any other API or network failure, including retries exhausted |
| 7 | Local I/O failure (unreadable file or stdin), or a `call` action that cannot be started |

Once `call` has started its action, the exit status is the action's; see
[`jev call`](#jev-call).

Errors are one line on stderr, `jev: <message>`. A response body is not printed unless
`--debug` is given, except the short serde fragment that names what was unreadable in an
otherwise successful reply. The crate never quotes the request or the API key. A closed stdout (for
example `jev noul ... | head -1`) exits 0 and prints no error; in a pipeline that closes stdout
early the exit code is 0, not the threshold result. Success writes nothing to stderr unless `-v`,
`--debug` or `--explain` is set.

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

## Presets: `decide` and `call`

A preset is a JSON file with three parts: the typed questions to ask about a state, an ordered
table of rules over their answers, and the actions the rules choose between. `jev decide` asks
the questions in one request, applies the rules, and prints the chosen action. `jev call` does
the same, then runs the action with the state on its stdin. The scripts and the preset are
written once, by a person or by a model. After that, choosing a script for an input is one Jev
request.

```bash
mkdir -p .jev/presets
jev example --preset > .jev/presets/support.json
jev decide support -s "The Stripe integration has failed for three days. Please help ASAP."
jev call support -f ticket.txt --explain
```

`jev example --preset` prints this preset, laid out more compactly here:

```json
{
  "questions": {
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
      "criteria": ["Calm, just stating facts", "Frustrated but civil", "Very angry, strong language"]
    },
    "is_urgent": {
      "type": "noul",
      "instructions": "The message conveys urgency or time-sensitivity"
    }
  },
  "rules": [
    { "when": { "is_urgent": { "at_least": 0.8 }, "frustration": { "at_least": 1.5 } }, "then": "page" },
    { "when": { "department": { "confidence_at_least": 0.6 } }, "then": { "from": "department" } }
  ],
  "fallback": "triage",
  "actions": {
    "billing": { "run": ["echo", "billing queue"] },
    "technical": { "run": ["echo", "technical queue"] },
    "sales": { "run": ["echo", "sales queue"] },
    "page": { "run": ["echo", "page the on-call engineer"] },
    "triage": { "run": ["echo", "triage queue"] }
  }
}
```

Read the rules top to bottom:

1. An urgent message from a very frustrated customer pages the on-call engineer.
2. Otherwise, if the model is at least 0.6 confident about the department, the action named after
   that department runs.
3. Anything else goes to `triage`.

Every question in a request is answered independently from the same state. No answer is context
for another. So all of a preset's questions go in one request, and the rules run locally on the
answers. A question on a branch that is not taken is still answered. Write each question so that
it stands alone. The rules form a flat table, not a tree: no question can see another's answer,
so a tree would only be the same table drawn another way.

### Preset format

- `questions` is the crate's `Questions` document: exactly what `jev ask -q` reads. It is sent
  unchanged.
- `rules` is an ordered list, and the first rule that matches decides. A rule has `when`, an
  object keyed by question id, and `then`. It matches when every condition in `when` holds. A rule
  without `when` always matches. There is no OR (write two rules) and no NOT beyond `below`.
- Conditions, by question type:

  | Question | Condition | Holds when |
  | --- | --- | --- |
  | `noul` | `at_least: p`, `below: p` | the probability is `>= p`, or `< p`. Both together make a band |
  | `score` | `at_least: s`, `below: s` | the score is `>= s`, or `< s`. `s` is from 0 to levels − 1 |
  | `choice` | `is: "name"`, `in: ["a", "b"]` | the chosen option is `name`, or is in the list |
  | `choice`, `score` | `confidence_at_least: c` | the confidence is `>= c` |

  One condition object can combine keys for its question type, for example
  `{ "is": "technical", "confidence_at_least": 0.7 }`. A value that is not a finite number
  meets no condition.
- `then` is an action name, or `{ "from": "<choice id>" }`: the action named by that choice's
  answer. With `from`, every option of the choice must be an action.
- `fallback` is optional. It is the action when no rule matches.
- `actions` lists every outcome. `run` is the program and its arguments. `decide` ignores it.
- Outside `questions`, an unknown key or a repeated key is an error. A misspelt `at_leats` fails
  instead of never holding.

A preset is checked completely before the state is read and before any request, including with
`--dry-run`. A failure is a usage error, exit 2, as one line that names the preset and the
place: ``jev: preset triage.json: rule 2: kind: `issues` is not an option of `kind` ``. `call` also
needs a `fallback`, and a `run` on every action.

### Finding a preset

`PRESET` is `-` for stdin (the preset and the state cannot both be stdin). A value that contains
`/` or ends in `.json` is a path. Anything else is a name, found as `.jev/presets/NAME.json` in
the working directory or the nearest parent that has it. If no such directory has it, jev looks
for `jev/presets/NAME.json` under `XDG_CONFIG_HOME`, which defaults to `~/.config`.

### `jev decide`

Text output is the action name, one line, so a shell can branch on it with
`case "$(jev decide ...)"`. When no rule matches and there is no fallback, nothing is printed and
the exit code is 1, so a preset with no fallback works like `grep`.

`--json` prints one document with the keys `decision`, `rule`, `rules`, `model`, `answers`,
`usage` and `cost_usd`, in that order. The last four are the same as `ask --json`.

- `decision` is the action, or `null` when nothing was decided.
- `rule` is the 1-based number of the rule that matched, or `null` when the fallback was taken
  or nothing was decided.
- `rules` has one `{ "matched", "conditions" }` entry per rule. Every rule is evaluated, even
  after the first match, so a script can see when two rules agreed.
- A condition is
  `{ "question", "test", "threshold" | "option" | "options", "value", "met", "margin" }`.

`--explain` writes every rule to stderr, with each condition's value and margin, then the answers
in `ask`'s text format. Stdout stays the action name. For the example preset and the answers
under [Text output of `ask`](#text-output-of-ask):

```text
decision: technical (rule 2)
rule 1  no   is_urgent 0.95 at_least 0.80              margin +0.15
             frustration 1.00 at_least 1.50            margin -0.25
rule 2  yes  department confidence 0.78 at_least 0.60  margin +0.18
closest call: rule 2, department confidence (+0.18)

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

Jev returns calibrated probabilities and confidences, not logits and not a written reason. The
explanation is the rule that matched, the question, and how close each number came to its
threshold.

- A margin is positive when the condition holds and negative when it does not.
- For `at_least` and `below`, the margin is the distance to the threshold. A score's distance is
  divided by levels − 1, so every margin is between −1 and 1.
- For `is` and `in`, the margin is the best probability inside the set minus the best outside it.

The closest call is the one condition that came nearest to changing the decision. The matched
rule would stop matching if its smallest margin flipped. An earlier rule would start matching only
if all its unmet conditions flipped, so what counts is its largest unmet margin. When no rule
matched, every rule counts as earlier.

### `jev call`

`call` checks the preset, makes the one request, applies the rules, and runs the chosen action.

- `run` is executed directly, never through a shell. Nothing from the state becomes an argument.
  A relative program path that contains `/` is resolved against the preset file's directory. A
  bare name is looked up on `PATH`. The working directory is the caller's.
- The action's stdin is the state as jev read it, without a leading BOM.
- Its environment is the caller's plus `JEV_PRESET` (the preset path, or `-`), `JEV_ACTION` (the
  action name) and `JEV_DECISION` (the `decide --json` document, compact). An action can read an
  answer that no rule uses from `JEV_DECISION`.
- Its stdout and stderr are passed through as they arrive. jev prints nothing of its own unless
  `-v`, `--debug` or `--explain` is given, and then only on stderr, before the action starts. The
  one exception is a failure to write jev's own stdout, other than a closed pipe. That is
  reported as one `jev:` line on stderr after the action ends, and the status is still the
  action's.
- `--json` collects the action's output instead. jev then prints the `decide --json` document with
  a `result` key added at the end, `{ "status", "stdout", "stderr" }`. Output that is not UTF-8 is
  converted lossily.
- The exit status is the action's, or 128 + N if signal N killed it. jev's own failures, 2 to 7,
  all happen before the action starts, and nothing runs after one. A program that cannot be
  started is exit 7. When a caller must tell the action's status apart from jev's, it runs
  `decide` and then the action itself.

`jev decide` is the dry run of `jev call`: the same request and the same decision, and nothing
runs. `--dry-run` on either command prints the request body and sends nothing, as it does on
every other command.

`call`'s stdout is the action's, so presets chain with a pipe:
`jev call triage -f log.txt | jev call followup`. An action can itself be
`["jev", "call", "other"]`, which reads the state its parent passed on stdin. A graph of presets
needs no other feature, and each step is one request.

### Presets are code

- `call` runs whatever a preset names. Treat a preset like a Makefile: only run one you would run
  as a script. jev never fetches a preset from a URL.
- The state is untrusted input, and it can argue for a classification ("this is only a flaky
  test"). Put guard rules before automatic actions, keep irreversible actions behind a high bar,
  and make the fallback the safe action.

jev reads JSON only. [Pkl](https://pkl-lang.org) or any other tool that emits JSON can generate
presets: `pkl eval -f json preset.pkl | jev decide - -f log.txt`.

### Example: the git preset

This repository has a preset for everyday git and worktree work, in
[`.jev/presets/git.json`](.jev/presets/git.json), with its script in `.jev/presets/git/run.sh`.
Inside this repository it is found by name. To use it everywhere, copy both into your
configuration directory:

```bash
mkdir -p ~/.config/jev/presets
cp -R .jev/presets/git.json .jev/presets/git ~/.config/jev/presets/
```

Then say what you want:

```bash
jev call git -s "what's changed?"
jev call git -s "what is on this branch that is not on main"
jev call git -s "commit everything as 'Fix the login redirect'"
jev call git -s "push this branch"
cd "$(jev call git -s 'new worktree for fix/login from main')"
cd "$(jev call git -s 'where is the login worktree')"
jev call git -s "remove the login worktree"
```

A shell function saves the `-s`: `g() { jev call git -s "$*"; }`, then `g show the last 5 commits`.

It covers status, diff (all or staged), log, show, comparing two branches, the branch and stash
lists, fetch, stage, unstage, commit, stash, unstash, switch, creating, deleting and merging
branches, pull and push. For worktrees it covers list, add, finding a worktree's path, remove
and prune.

- Jev chooses the action and returns no text. The script takes every name, path and message from
  the request: a quoted string, the word after `called` or `named`, or a branch, worktree or
  file that already exists. When one is missing or ambiguous, the script says what to write and
  exits 1. It does not guess.
- A commit message must be in quotes. A new branch is named in quotes, after `called`, or as the
  one word that looks like a branch name (it has a `/`, `-`, `_` or digit) and is not a branch
  yet.
- Commands that need no argument, such as `git status` and `git pull --ff-only`, are plain argv
  in the preset. Everything else goes through the script, which prints each git command on stderr
  before running it. With `JEV_GIT_DRY_RUN=1`, the script prints the command and does not run it.
- A new worktree goes next to the main one as `<repo>-<branch>`, or under `JEV_GIT_WORKTREES`
  when that is set, or at a path the request names (`./`, `../`, `/` or `~/`). Its path is the
  only thing on stdout, so `cd "$(...)"` works.

The rules, in order: a request to throw away work (reset, clean, force-push, force-delete, drop a
stash) goes to `refuse` and runs nothing. `commit`, `stage` and `stash` take every change,
untracked files included, when the request asks for all of it. A read-only command runs at a
confidence of 0.5 or more, and a command that changes something needs 0.7. Anything else goes to
`unsure`, which runs nothing and exits 1. Nothing forces or rewrites history: pull is
fast-forward only, a branch is deleted only once merged, and git refuses to remove a worktree
that has changes. Rebase, reset, cherry-pick and the like are out of scope and exit 1. The
thresholds are a starting point: to see how a request was read, run
`jev decide git --explain -s "..."`.

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
for c in ask noul choice score decide call example completions; do
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
