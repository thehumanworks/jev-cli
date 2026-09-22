#!/usr/bin/env bash
# One-time (re-runnable) setup of the Modal-hosted CI runners described in docs/ci-runners.md.
#
# Needs: `modal` (signed in), `gh` (signed in with admin on the repository), `openssl`.
# Does: create the Modal secret, deploy ci/modal_runner.py, register the GitHub webhook, and
# point the workflow at the Modal runners by setting the CI_RUNNER repository variable.
#
# GITHUB_RUNNER_TOKEN, if set, is the token the runners use to register themselves; otherwise
# the token `gh` is signed in with. A fine-grained token with "Administration: write" on the
# repository is the least privilege that works.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

repo=thehumanworks/jev-cli
secret_name=jev-ci-runner
token=${GITHUB_RUNNER_TOKEN:-$(gh auth token)}
webhook_secret=$(openssl rand -hex 32)

echo "==> Modal secret $secret_name"
modal secret create "$secret_name" "GITHUB_TOKEN=$token" "WEBHOOK_SECRET=$webhook_secret" --force

echo "==> Deploying ci/modal_runner.py"
deploy_log=$(mktemp)
trap 'rm -f "$deploy_log"' EXIT
modal deploy ci/modal_runner.py 2>&1 | tee "$deploy_log"
url=$(grep -oE 'https://[a-z0-9.-]+\.modal\.run[^ ]*' "$deploy_log" | head -1 || true)
if [[ -z $url ]]; then
  echo "could not find the webhook URL in the deploy output" >&2
  exit 1
fi

echo "==> GitHub webhook -> $url"
# Replace any hook already pointing at this endpoint so its secret matches the new one.
for id in $(gh api "repos/$repo/hooks" --jq ".[] | select(.config.url == \"$url\") | .id"); do
  gh api -X DELETE "repos/$repo/hooks/$id"
done
gh api -X POST "repos/$repo/hooks" \
  -f name=web -F active=true -f 'events[]=workflow_job' \
  -f "config[url]=$url" -f config[content_type]=json -f config[insecure_ssl]=0 \
  -f "config[secret]=$webhook_secret" \
  --jq '"webhook \(.id) registered"'

echo "==> Selecting the Modal runners for CI"
gh variable set CI_RUNNER --repo "$repo" --body modal

echo
echo "Done. CI jobs now run on Modal. To go back to GitHub-hosted runners:"
echo "  gh variable delete CI_RUNNER --repo $repo"
