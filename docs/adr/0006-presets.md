# ADR 0006: Presets, `decide` and `call`

- Status: Accepted
- Date: 2026-09-23

## Context

A person or a capable model can write scripts, and can write down when each one applies. Once
both are written, choosing a script for a new input (a CI log, a ticket, an agent's proposed tool
call) should not need that model again. Jev is a good fit for that choice, for three reasons:

- It answers typed questions with calibrated probabilities, so a threshold means something.
- It answers every question in a request independently, from the same state, in about the time
  of one.
- It charges only for input tokens.

It also sets limits. It returns no text, so it can choose between actions but cannot fill in
their arguments. It returns probabilities and confidences, not logits and not reasons. And one
answer is never context for another.

## Decision

A preset is one JSON file: the crate's `Questions` document under `questions`, unchanged, plus a
first-match rule table and a set of actions. Only the rules and actions are this program's own
format.

- **One request per decision.** All of a preset's questions are sent together, and the rules run
  locally over the answers. A decision tree was rejected. No question can see another's answer,
  so a tree adds no information over a table. Asking only the questions on the taken branch
  would save tokens that cost almost nothing, and would add a round trip per level.
- **Rules are a flat, ordered table.** The first rule whose conditions all hold decides. The
  conditions are a closed set per question type: `at_least`, `below`, `is`, `in` and
  `confidence_at_least`. `then` names an action, or takes it from a choice's answer (`from`). An
  optional `fallback` catches everything else. There is no OR, no NOT and no arithmetic. A
  general expression language would be a second program to test, and two rules cover an OR.
- **Unknown and repeated keys are errors.** Outside `questions`, unknown keys and repeated keys
  are rejected, and the whole preset is checked before the state is read. A typo must fail
  before it costs a request, not become a rule that never matches.
- **Deciding and running are separate commands.** `decide` prints the action and runs nothing.
  It is `call`'s dry run and is fully testable in-process. `call` runs the action. `--dry-run`
  keeps its meaning on both: print the request body, send nothing.
- **An action is an argv, never a shell command.** The state goes to the action's stdin and
  never into its arguments. The decision goes in `JEV_DECISION`. A relative program path is
  resolved against the preset's directory, so a preset and its scripts can move together.
- **`call` spawns the action and waits for it; it does not `exec`.** Spawning keeps
  `jev::run` testable with in-memory writers, and lets `--json` collect the output. The status
  is passed through, as `env` and `nice` pass it through, with 128 + N for a signal. jev's own
  failures keep 2–7 and happen before the action starts. A caller that must tell the two apart
  runs `decide`.
- **`decide` exits 1 when nothing matched and there is no fallback**, like `grep` and
  `noul --threshold`. `call` requires a fallback, so it always runs something.
- **`--explain` goes to stderr.** It shows each condition's value and margin, then the closest
  call, which is the condition nearest to changing the decision. These numbers are the
  explanation Jev supports.
- **Named presets** are `.jev/presets/NAME.json`, found from the working directory upwards, then
  under `$XDG_CONFIG_HOME/jev/presets/`. A value with `/` or ending in `.json` is always a path,
  so a name never silently shadows a file.
- **jev reads JSON only.** Pkl and other generators emit JSON. An embedded evaluator would be a
  dependency for an authoring convenience.

## Consequences

A graph of decisions is presets whose actions call `jev call` on another preset, or a pipe
between two `call`s. Each step is one request. Nothing in the binary models the graph.

Every question is answered for every state, including questions whose rule is never reached. A
question must be written to stand alone. The action receives the whole decision, so a question
that no rule uses can still pass a value to the script.

A preset is code: `call` runs what it names, and the state is untrusted input that can argue for
a classification. The README says to put guards before automatic actions and to make the
fallback the safe action.

The JSON documents of `decide` and `call`, the text of `decide`, the exit codes, and the
environment given to an action are contracts, pinned by tests like the rest of ADR 0002.

## Open

- Is the state billed once per request, or once per question? One live pair would settle it:
  the same state with 1 question and with 10, comparing `input_tokens`. That pair spends credits
  and sends the state to TypeSafe, so it runs only when asked. The answer decides whether a large
  preset needs a cost warning.
- How far can a state move the answers by arguing for one? Build a small adversarial set before
  any preset runs something irreversible.
