# CI runners on Modal

CI (`.github/workflows/ci.yml`) runs `scripts/check.sh` and the x86_64 Linux release build on a
runner chosen by the `CI_RUNNER` repository variable:

| `CI_RUNNER` | Runner |
| --- | --- |
| unset | GitHub-hosted `ubuntu-latest` |
| `modal` | An ephemeral container on Modal, one per job, from `ci/modal_runner.py` |

Switch with `gh variable set CI_RUNNER --body modal` or `gh variable delete CI_RUNNER`. The
workflow itself does not change.

## How the Modal runners work

1. GitHub posts a `workflow_job` webhook to the `webhook` endpoint of the Modal app
   `jev-ci-runner` every time a job changes state.
2. For a `queued` job whose labels include `modal`, the endpoint spawns `run_job`: a fresh
   container with the Actions runner, git, a C toolchain, musl-tools and Rust 1.98.1 (with clippy,
   rustfmt and the `x86_64-unknown-linux-musl` target) baked into the image.
3. The container asks GitHub for a single-use just-in-time runner registration, starts the runner
   with it, runs that one job, and exits. Nothing is shared between jobs, and no runner is
   registered while idle.

Each container gets 4 CPUs, 8 GiB of memory and 40 minutes. The job's own `timeout-minutes` in
the workflow is shorter and is what normally applies.

The aarch64 Linux and macOS builds always run on GitHub-hosted runners (`ubuntu-24.04-arm`,
`macos-latest`); Modal has no arm64 or macOS containers. The release job runs on
`ubuntu-latest` because it needs `gh`.

## Setup

```bash
ci/setup-modal-runner.sh
```

The script is re-runnable. It creates the Modal secret `jev-ci-runner` (`GITHUB_TOKEN` with
admin on the repository, `WEBHOOK_SECRET`), deploys the app, registers the webhook with that
secret, and sets `CI_RUNNER=modal`. Set `GITHUB_RUNNER_TOKEN` to use a dedicated fine-grained
token ("Administration: write" on this repository) instead of the token `gh` is signed in with.

After changing `ci/modal_runner.py`, `modal deploy ci/modal_runner.py` is enough; the webhook
URL is stable across deploys.

## Operating

- Runs: `gh run list`; the job page shows which runner (`modal-<job id>`) took it.
- Container logs: `modal app logs jev-ci-runner`.
- Webhook deliveries and redelivery: repository settings, Webhooks, or
  `gh api repos/thehumanworks/jev-cli/hooks/<id>/deliveries`.
- A job stuck in "Waiting for a runner" means the webhook did not reach Modal or `run_job`
  failed before registering: check the delivery response and the app logs. Redeliver the
  webhook, or set `CI_RUNNER` back to unset and re-run the job on GitHub.

## Security notes

- The runner container holds `GITHUB_TOKEN` only until it has minted the just-in-time config;
  both secrets are removed from the environment before the runner starts, so job steps and
  pull-request code cannot read them. The just-in-time registration is valid for one job.
- The webhook is verified with an HMAC (`X-Hub-Signature-256`); events for other repositories
  or without the `modal` label are ignored.
- The repository is public. GitHub requires approval before workflows run for first-time
  contributors, and every job runs in a throwaway container, but a pull request from a fork
  still runs its code on infrastructure you pay for. Keep the approval requirement on.
