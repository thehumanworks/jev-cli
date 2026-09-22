"""Ephemeral GitHub Actions runners for this repository, hosted on Modal.

How it works:

1. GitHub sends a `workflow_job` webhook to the `webhook` endpoint below whenever a job is queued.
2. If the job asks for the `modal` label, `run_job` is spawned: one fresh container per job.
3. The container asks GitHub for a single-use just-in-time runner config, starts the Actions
   runner with it, runs exactly that one job, and exits. Nothing persists between jobs.

Deploy and wire it up with the commands in `docs/ci-runners.md`. The Modal secret
`jev-ci-runner` must hold `GITHUB_TOKEN` (repository admin, to mint runner configs) and
`WEBHOOK_SECRET` (shared with the GitHub webhook). Both are removed from the environment before
the runner starts, so job steps never see them.
"""

import hashlib
import hmac
import json
import os
import subprocess
import urllib.request

import modal

REPO = "thehumanworks/jev-cli"
LABEL = "modal"
RUNNER_VERSION = "2.337.0"
RUST_VERSION = "1.98.1"
SECRET_NAME = "jev-ci-runner"
SECRET_KEYS = ("GITHUB_TOKEN", "WEBHOOK_SECRET")
# Upper bound for one job. `scripts/check.sh` takes a few minutes cold; a runner that never gets
# a job (the queued job was taken by someone else) is also stopped by this.
JOB_TIMEOUT_SECONDS = 40 * 60

app = modal.App("jev-ci-runner")
secret = modal.Secret.from_name(SECRET_NAME, required_keys=list(SECRET_KEYS))

runner_tarball = (
    f"https://github.com/actions/runner/releases/download/v{RUNNER_VERSION}/"
    f"actions-runner-linux-x64-{RUNNER_VERSION}.tar.gz"
)

# Everything a job needs is baked in, so a cold container starts a job in seconds:
# the Actions runner, git, a C toolchain for build scripts, and the pinned Rust toolchain
# that `rust-toolchain.toml` and `.github/workflows/ci.yml` name.
runner_image = (
    modal.Image.debian_slim(python_version="3.12")
    .apt_install("curl", "git", "ca-certificates", "build-essential", "pkg-config", "tar", "gzip")
    .run_commands(
        f"mkdir -p /runner && curl -fsSL {runner_tarball} | tar -xz -C /runner",
        "/runner/bin/installdependencies.sh",
        "curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal "
        f"--default-toolchain {RUST_VERSION} -c clippy -c rustfmt",
    )
    .env(
        {
            "PATH": "/root/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "RUNNER_ALLOW_RUNASROOT": "1",
            "HOME": "/root",
        }
    )
)

webhook_image = modal.Image.debian_slim(python_version="3.12").pip_install("fastapi[standard]")


def github(path: str, token: str, body: dict) -> dict:
    """One POST to the GitHub REST API for this repository."""
    request = urllib.request.Request(
        f"https://api.github.com/repos/{REPO}/{path}",
        data=json.dumps(body).encode(),
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


@app.function(
    image=runner_image,
    secrets=[secret],
    timeout=JOB_TIMEOUT_SECONDS,
    cpu=4,
    memory=8192,
    ephemeral_disk=20 * 1024,
)
def run_job(job_id: int, job_url: str) -> None:
    """Run exactly one queued job in this container, then exit."""
    token = os.environ["GITHUB_TOKEN"]
    config = github(
        "actions/runners/generate-jitconfig",
        token,
        {"name": f"modal-{job_id}", "runner_group_id": 1, "labels": [LABEL], "work_folder": "_work"},
    )["encoded_jit_config"]
    # Job steps inherit the runner's environment. Strip the secrets first so a pull request
    # cannot read them; the just-in-time config is single-use and scoped to this one job.
    env = {key: value for key, value in os.environ.items() if key not in SECRET_KEYS}
    print(f"starting runner modal-{job_id} for {job_url}")
    subprocess.run(["./run.sh", "--jitconfig", config], cwd="/runner", env=env, check=True)
    print(f"runner modal-{job_id} finished")


@app.function(image=webhook_image, secrets=[secret])
@modal.fastapi_endpoint(method="POST")
async def webhook(request) -> dict:
    """Receive GitHub's `workflow_job` events and spawn one runner per queued job."""
    from fastapi import HTTPException

    body = await request.body()
    expected = "sha256=" + hmac.new(os.environ["WEBHOOK_SECRET"].encode(), body, hashlib.sha256).hexdigest()
    if not hmac.compare_digest(expected, request.headers.get("X-Hub-Signature-256", "")):
        raise HTTPException(status_code=401, detail="bad signature")
    event = request.headers.get("X-GitHub-Event", "")
    if event != "workflow_job":
        return {"ignored": event}
    payload = json.loads(body)
    job = payload.get("workflow_job") or {}
    if (
        payload.get("action") != "queued"
        or LABEL not in job.get("labels", [])
        or payload.get("repository", {}).get("full_name") != REPO
    ):
        return {"ignored": payload.get("action"), "job": job.get("id")}
    call = run_job.spawn(job["id"], job.get("html_url", ""))
    return {"spawned": job["id"], "call": call.object_id}
