#!/usr/bin/env bash
# Install the repository's pre-commit hook. It runs scripts/pre-commit.sh.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
git rev-parse --is-inside-work-tree >/dev/null
chmod +x "$root/scripts/pre-commit.sh" "$root/scripts/check.sh" "$root/scripts/install-git-hooks.sh"
hook="$root/.git/hooks/pre-commit"
cat >"$hook" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
exec "$root/scripts/pre-commit.sh"
EOF
chmod +x "$hook"
printf 'Installed %s\n' "$hook"
printf 'Skip one commit with JEV_SKIP_HOOKS=1 or SKIP=1.\n'
