#!/usr/bin/env bash
# The actions of the git preset (../git.json) that need more than a fixed argv.
#
# Usage: run.sh <action> [--all | --staged], with the request on stdin.
#
# Jev chooses the action and returns no text, so every name, path and message here comes from the
# request itself: a quoted string, or a word that already exists in the repository (a branch, a
# worktree, a file). When that is missing or ambiguous, the action prints what to say instead and
# exits 1. Nothing here forces, discards work or rewrites history.
#
# Every git command that runs is printed to stderr first. JEV_GIT_DRY_RUN=1 prints it without
# running it. A new worktree goes next to the main one as <repo>-<branch>, or under
# JEV_GIT_WORKTREES when that is set, or at a path the request names (./, ../, / or ~/).
#
# Written for bash 3.2, the stock macOS shell; CI runs it under a newer bash on Linux only.
set -euo pipefail

request=$(cat)
# Nothing below reads stdin: git gets no chance to consume or wait on it.
exec </dev/null

say() { printf 'jev git: %s\n' "$*" >&2; }

# Print a reason and, optionally, what to say instead. Exit 1.
fail() {
  say "$1"
  if [[ $# -gt 1 ]]; then
    say "for example: jev call git -s '$2'"
  fi
  exit 1
}

run() {
  printf '+' >&2
  printf ' %q' "$@" >&2
  printf '\n' >&2
  if [[ -z ${JEV_GIT_DRY_RUN:-} ]]; then
    "$@"
  fi
}

in_repo() {
  git rev-parse --git-dir >/dev/null 2>&1 || fail "not inside a git repository"
}

# --- Reading the request -------------------------------------------------------------------------

# Patterns for the quoted strings of a request, in the order they are tried. A single-quoted string
# must open after a space and close before one, so that apostrophes (don't, main's) are not quotes.
# shellcheck disable=SC2016 # the backticks are a pattern to match, not a command
patterns=('"([^"]+)"' '`([^`]+)`' '“([^”]+)”' "(^|[[:space:]])'([^']+)'($|[[:space:][:punct:]])")

# The first quoted string, or nothing.
quoted() {
  local pattern
  for pattern in "${patterns[@]}"; do
    if [[ $request =~ $pattern ]]; then
      if [[ ${#BASH_REMATCH[@]} -gt 3 ]]; then
        printf '%s' "${BASH_REMATCH[2]}"
      else
        printf '%s' "${BASH_REMATCH[1]}"
      fi
      return
    fi
  done
}

# The request without its quoted strings: what is left once a commit message is taken out.
unquoted() {
  local text=$request pattern
  for pattern in "${patterns[@]}"; do
    while [[ $text =~ $pattern ]]; do
      text=${text/"${BASH_REMATCH[0]}"/ }
    done
  done
  printf '%s' "$text"
}

# The words of a text, one per line, without quote marks or the punctuation around them.
words() {
  local text=$1 word
  local -a all
  text=${text//\"/ }
  text=${text//\`/ }
  text=${text//“/ }
  text=${text//”/ }
  text=${text//’/\'}
  read -r -d '' -a all <<<"$text" || true
  for word in ${all[@]+"${all[@]}"}; do
    while [[ $word == [\(\[\{\<\']* ]]; do word=${word:1}; done
    word=${word%\'s}
    while [[ $word == *[.,\;:!?\)\]\}\>\'] ]]; do word=${word%?}; done
    if [[ -n $word ]]; then
      printf '%s\n' "$word"
    fi
  done
}

# The words of the request that are lines of $1, each once, in the order the request has them.
mentioned() {
  local candidates=$1 word seen=$'\n'
  while IFS= read -r word; do
    if [[ $seen != *$'\n'"$word"$'\n'* ]] && grep -Fxq -- "$word" <<<"$candidates"; then
      printf '%s\n' "$word"
      seen+="$word"$'\n'
    fi
  done < <(words "$request")
}

# The lines of $1 on one line, separated by spaces.
joined() { printf '%s' "$1" | tr '\n' ' '; }

# The single line of $2, or fail. $1 names what was looked for; $3 is an example request.
exactly_one() {
  local what=$1 found=$2 example=$3
  if [[ -z $found ]]; then
    fail "the request names no $what" "$example"
  fi
  if [[ $found == *$'\n'* ]]; then
    fail "the request names more than one $what: $(joined "$found")" "$example"
  fi
  printf '%s' "$found"
}

# Set `paths` to the words of $1 that are files or directories here, or that git knows (a deleted
# file). An array, because a path may contain anything but a newline.
paths=()
find_paths() {
  local word
  paths=()
  while IFS= read -r word; do
    if [[ -e $word ]] || git ls-files --error-unmatch -- "$word" >/dev/null 2>&1; then
      paths+=("$word")
    fi
  done < <(words "$1")
}

# --- The repository ------------------------------------------------------------------------------

local_branches() { git for-each-ref --format='%(refname:short)' refs/heads; }

# Remote branches as origin/name, without origin/HEAD.
remote_branches() {
  git for-each-ref --format='%(refname)' refs/remotes | sed -e 's|^refs/remotes/||' -e '/\/HEAD$/d'
}

# Remote branches without their remote: what `git switch name` would create a tracking branch for.
remote_names() { remote_branches | sed -e 's|^[^/]*/||'; }

current_branch() { git symbolic-ref --quiet --short HEAD || true; }

# Lines of $1 that are not the current branch.
not_current() {
  local current
  current=$(current_branch)
  grep -Fxv -- "$current" <<<"$1" || true
}

# Every worktree as "path<TAB>branch", the main one first. A detached worktree has no branch.
worktrees() {
  git worktree list --porcelain | awk '
    /^worktree / { if (path != "") print path "\t" branch; path = substr($0, 10); branch = "" }
    /^branch / { branch = substr($0, 8); sub(/^refs\/heads\//, "", branch) }
    END { if (path != "") print path "\t" branch }'
}

# The paths of the worktrees the request names by branch, by the branch's last part (login for
# feature/login), or by directory name.
named_worktrees() {
  local path branch mentions
  mentions=$(words "$request")
  while IFS=$'\t' read -r path branch; do
    local key
    for key in "$branch" "${branch##*/}" "${path##*/}"; do
      if [[ -n $key ]] && grep -Fxq -- "$key" <<<"$mentions"; then
        printf '%s\n' "$path"
        break
      fi
    done
  done < <(worktrees)
}

# A name for something new: the first quoted string, the word after "called" or "named", or else
# the one word that looks like a branch name (it has a /, -, _ or digit) and is not one yet.
new_name() {
  local name existing word previous="" candidates=""
  name=$(quoted)
  if [[ -n $name ]] && git check-ref-format --branch "$name" >/dev/null 2>&1; then
    printf '%s' "$name"
    return
  fi
  existing=$(printf '%s\n%s\n' "$(local_branches)" "$(remote_names)")
  while IFS= read -r word; do
    case $previous:$word in
      called:a | called:an | called:the | named:a | named:an | named:the | named:after) ;;
      called:* | named:*)
        if git check-ref-format --branch "$word" >/dev/null 2>&1; then
          printf '%s' "$word"
          return
        fi
        ;;
    esac
    previous=$word
    if [[ $word == *[/_0-9-]* && $word != [/.~]* ]] &&
      ! grep -Fxq -- "$word" <<<"$existing" &&
      git check-ref-format --branch "$word" >/dev/null 2>&1; then
      candidates+="$word"$'\n'
    fi
  done < <(words "$(unquoted)")
  candidates=${candidates%$'\n'}
  if [[ -n $candidates && $candidates != *$'\n'* ]]; then
    printf '%s' "$candidates"
  fi
}

# The remote to push a new branch to: the only one, or origin.
push_remote() {
  local remotes
  remotes=$(git remote)
  if [[ -n $remotes && $remotes != *$'\n'* ]]; then
    printf '%s' "$remotes"
  elif grep -Fxq origin <<<"$remotes"; then
    printf 'origin'
  else
    fail "cannot tell which remote to push to: $(joined "$remotes")"
  fi
}

# The branch another branch is compared against when the request names none.
default_branch() {
  local head
  if head=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null); then
    printf '%s' "$head"
    return
  fi
  for head in main master trunk; do
    if git show-ref --verify --quiet "refs/heads/$head"; then
      printf '%s' "$head"
      return
    fi
  done
}

# --- Actions -------------------------------------------------------------------------------------

diff() {
  local -a args=()
  if [[ ${1:-} == --staged ]]; then
    args+=(--staged)
  fi
  find_paths "$request"
  run git diff ${args[@]+"${args[@]}"} -- ${paths[@]+"${paths[@]}"}
}

log() {
  local count=20 word ref
  local -a args=(--oneline --decorate)
  while IFS= read -r word; do
    if [[ $word =~ ^[0-9]{1,4}$ ]] && ((10#$word > 0)); then
      count=$word
      break
    fi
  done < <(words "$request")
  args+=(-n "$count")
  ref=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_branches)")" | sed -n 1p)
  if [[ -n $ref ]]; then
    args+=("$ref")
  fi
  find_paths "$request"
  run git log "${args[@]}" -- ${paths[@]+"${paths[@]}"}
}

show() {
  local word ref=HEAD
  while IFS= read -r word; do
    if [[ $word =~ ^[0-9a-f]{4,40}$ || $word =~ ^HEAD([~^][0-9]*)*$ || $word =~ ^(ORIG_|FETCH_)HEAD$ ]] ||
      git show-ref --quiet --tags --heads -- "$word"; then
      if git rev-parse --verify --quiet "$word^{commit}" >/dev/null; then
        ref=$word
        break
      fi
    fi
  done < <(words "$request")
  run git show --stat --patch "$ref"
}

# One branch named: it against the current one. Two: the first against the second. None: the current
# branch against the default one. `<` marks commits only on the first side, `>` only on the second.
compare() {
  local refs left right
  refs=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_branches)")")
  case $(grep -c . <<<"$refs" || true) in
    0)
      left=$(default_branch)
      [[ -n $left ]] || fail "the request names no branch to compare with" 'what is on this branch that is not on main'
      right=HEAD
      ;;
    1)
      left=$refs
      right=HEAD
      ;;
    2)
      left=$(sed -n 1p <<<"$refs")
      right=$(sed -n 2p <<<"$refs")
      ;;
    *) fail "the request names more than two branches: $(joined "$refs")" 'compare feature/login with main' ;;
  esac
  if [[ $right == HEAD && $left == "$(current_branch)" ]]; then
    fail "that is the current branch; name the branch to compare it with" 'compare this branch with main'
  fi
  run git log --oneline --decorate --left-right "$left...$right"
  run git diff --stat "$left...$right"
}

stage() {
  if [[ ${1:-} == --all ]]; then
    run git add --all
    return
  fi
  find_paths "$request"
  [[ ${#paths[@]} -gt 0 ]] || fail "the request names no file to stage" 'stage src/main.rs'
  run git add -- "${paths[@]}"
}

# Named files, or everything. The changes stay in the files either way.
unstage() {
  find_paths "$request"
  if [[ ${#paths[@]} -eq 0 ]]; then
    paths=(:/)
  fi
  run git restore --staged -- "${paths[@]}"
}

# The message is the first quoted string. Files named outside it are staged and committed alone.
commit() {
  local message
  message=$(quoted)
  [[ -n $message ]] || fail "put the commit message in quotes" 'commit everything as "Fix the login redirect"'
  find_paths "$(unquoted)"
  if [[ ${#paths[@]} -gt 0 ]]; then
    run git add -- "${paths[@]}"
    run git commit --message "$message" -- "${paths[@]}"
    return
  fi
  if [[ ${1:-} == --all ]]; then
    run git add --all
  fi
  run git commit --message "$message"
}

stash() {
  local message
  local -a args=(push)
  if [[ ${1:-} == --all ]]; then
    args+=(--include-untracked)
  fi
  message=$(quoted)
  if [[ -n $message ]]; then
    args+=(--message "$message")
  fi
  run git stash "${args[@]}"
}

unstash() {
  local pattern='stash@\{[0-9]+\}'
  if [[ $request =~ $pattern ]]; then
    run git stash pop "${BASH_REMATCH[0]}"
  else
    run git stash pop
  fi
}

switch() {
  local mentions target
  mentions=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_names)" | sort -u)")
  if [[ -n $mentions && $mentions == "$(current_branch)" ]]; then
    fail "already on $mentions"
  fi
  target=$(exactly_one "branch" "$(not_current "$mentions")" 'switch to main')
  run git switch "$target"
}

create_branch() {
  local name base
  name=$(new_name)
  [[ -n $name ]] || fail "put the new branch's name in quotes" 'create a branch "fix/login-redirect" from main'
  base=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_branches)")" | sed -n 1p)
  if [[ -n $base ]]; then
    run git switch --create "$name" "$base"
  else
    run git switch --create "$name"
  fi
}

delete_branch() {
  local target
  target=$(exactly_one "branch" "$(not_current "$(mentioned "$(local_branches)")")" 'delete the branch fix/typo')
  run git branch --delete "$target"
}

merge() {
  local target
  target=$(exactly_one "branch" "$(not_current "$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_branches)")")")" \
    'merge main into this branch')
  run git merge --no-edit "$target"
}

push() {
  local branch remote
  branch=$(current_branch)
  [[ -n $branch ]] || fail "HEAD is detached; switch to a branch first"
  if git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' >/dev/null 2>&1; then
    run git push
  else
    remote=$(push_remote)
    run git push --set-upstream "$remote" "$branch"
  fi
}

# The directory for a new worktree of branch $1.
worktree_dir() {
  local word main
  while IFS= read -r word; do
    case $word in
      ./* | ../* | /*)
        printf '%s' "$word"
        return
        ;;
      \~/*)
        printf '%s' "$HOME/${word#\~/}"
        return
        ;;
    esac
  done < <(words "$request")
  if [[ -n ${JEV_GIT_WORKTREES:-} ]]; then
    printf '%s/%s' "${JEV_GIT_WORKTREES%/}" "${1//\//-}"
    return
  fi
  main=$(worktrees | sed -n 1p | cut -f 1)
  printf '%s/%s-%s' "$(dirname "$main")" "$(basename "$main")" "${1//\//-}"
}

# A new branch when the request names one (from a base branch it also names), else an existing
# branch that no worktree has checked out. Print the new worktree's path, and nothing else, on
# stdout, so that `cd "$(jev call git -s ...)"` goes there.
worktree_add() {
  local name mentions checked_out existing path base
  name=$(new_name)
  if [[ -n $name ]]; then
    path=$(worktree_dir "$name")
    base=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_branches)")" | sed -n 1p)
    run git worktree add -b "$name" "$path" ${base:+"$base"} >&2
  else
    mentions=$(mentioned "$(printf '%s\n%s\n' "$(local_branches)" "$(remote_names)" | sort -u)")
    checked_out=$(worktrees | cut -f 2)
    existing=$(grep -Fxv -f <(printf '%s\n' "$checked_out") <<<"$mentions" || true)
    if [[ -z $existing && -n $mentions ]]; then
      fail "$(joined "$mentions") is already checked out; to go there: jev call git -s 'where is the worktree for $(sed -n 1p <<<"$mentions")'"
    fi
    name=$(exactly_one "branch" "$existing" 'new worktree for "feature/login"')
    path=$(worktree_dir "$name")
    run git worktree add "$path" "$name" >&2
  fi
  printf '%s\n' "$path"
}

worktree_path() {
  exactly_one "worktree" "$(named_worktrees)" 'where is the worktree for feature/login'
  printf '\n'
}

# Never the main worktree or the current one. Without --force, git refuses a worktree with changes.
worktree_remove() {
  local named main here target branch
  named=$(named_worktrees)
  main=$(worktrees | sed -n 1p | cut -f 1)
  here=$(git rev-parse --show-toplevel)
  target=$(grep -Fxv -e "$main" -e "$here" <<<"$named" || true)
  if [[ -z $target && -n $named ]]; then
    fail "that is the main worktree or the one you are in; neither is removed"
  fi
  target=$(exactly_one "worktree" "$target" 'remove the worktree for feature/login')
  branch=$(worktrees | awk -F '\t' -v path="$target" '$1 == path { print $2 }')
  run git worktree remove "$target"
  if [[ -n $branch ]]; then
    say "the branch $branch is kept; once it is merged: jev call git -s 'delete the branch $branch'"
  fi
}

help() {
  cat <<'EOF'
Say what you want in plain words: jev call git -s '...'

Look       status, diff (staged), log, show a commit, compare branches, branches, stashes, fetch
Change     stage, unstage, commit (message in quotes), stash, unstash
Branches   switch, create (from a base), delete a merged branch, merge, pull (fast-forward), push
Worktrees  list, add (new or existing branch), find a worktree's path, remove, prune

Names come from the request: a quoted string, or a branch, worktree or file that exists.
Nothing forces, discards work or rewrites history. To see why a request was read as it was:
  jev decide git --explain -s '...'
EOF
}

case ${1:-} in
  help)
    help
    ;;
  unsure)
    say "not sure what that asks for; nothing ran. To see how it was read:"
    say "  jev decide git --explain -s '...'"
    help >&2
    exit 1
    ;;
  refuse)
    fail "this preset does not discard work, rewrite history or force; run git yourself if you mean it"
    ;;
  unsupported)
    fail "that is not one of the operations this preset runs; run git yourself, or ask for help" 'what can you do'
    ;;
  diff | log | show | compare | stage | unstage | commit | stash | unstash | switch | create_branch | \
    delete_branch | merge | push | worktree_add | worktree_path | worktree_remove)
    in_repo
    action=$1
    shift
    "$action" "$@"
    ;;
  *)
    say "unknown action: ${1:-}"
    exit 2
    ;;
esac
