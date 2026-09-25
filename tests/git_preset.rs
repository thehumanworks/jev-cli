//! The git preset in `.jev/presets`: its actions run against throwaway repositories, and one
//! decision goes through `jev call` with a local fake server. Nothing here touches the network: the
//! remote is a bare repository in the same temporary directory.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const PRESET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.jev/presets/git.json");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/.jev/presets/git/run.sh");

/// A repository on `main` with one pushed commit, the branches `feature/login` and `fix-typo`, and
/// a bare `origin` beside it. The developer's git configuration is never read.
struct Repo {
    _root: tempfile::TempDir,
    path: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repo");
        let repo = Self { path, _root: root };
        git_in(repo.parent(), &["init", "--quiet", "--bare", "remote.git"]);
        git_in(repo.parent(), &["init", "--quiet", "--initial-branch", "main", "repo"]);
        repo.git(&["remote", "add", "origin", "../remote.git"]);
        repo.write("a.txt", "a\n");
        repo.write("src/b.rs", "b\n");
        repo.git(&["add", "--all"]);
        repo.git(&["commit", "--quiet", "--message", "init"]);
        repo.git(&["push", "--quiet", "--set-upstream", "origin", "main"]);
        repo.git(&["branch", "feature/login"]);
        repo.git(&["branch", "fix-typo"]);
        repo
    }

    fn parent(&self) -> &Path {
        self.path.parent().unwrap()
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.path.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        git_in(&self.path, args)
    }

    /// Run `run.sh ACTION...` here with `request` on stdin.
    fn act(&self, action: &[&str], request: &str) -> Acted {
        self.act_with(action, request, &[])
    }

    fn act_with(&self, action: &[&str], request: &str, env: &[(&str, &str)]) -> Acted {
        let mut command = isolated(Command::new(SCRIPT));
        command.current_dir(&self.path).args(action).envs(env.iter().copied());
        Acted::from(feed(command, request))
    }

    fn branch(&self) -> String {
        self.git(&["branch", "--show-current"])
    }

    fn has_branch(&self, name: &str) -> bool {
        !self.git(&["branch", "--list", name]).is_empty()
    }
}

/// Run git in `dir` and return its stdout, trimmed. Any failure fails the test.
fn git_in(dir: &Path, args: &[&str]) -> String {
    let output = isolated(Command::new("git")).current_dir(dir).args(args).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim_end().to_owned()
}

/// A command that sees none of the developer's git configuration, with a fixed identity.
fn isolated(mut command: Command) -> Command {
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::temp_dir())
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

fn feed(mut command: Command, stdin: &str) -> Output {
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

struct Acted {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl From<Output> for Acted {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).unwrap(),
            stderr: String::from_utf8(output.stderr).unwrap(),
        }
    }
}

impl Acted {
    #[track_caller]
    fn ok(self) -> Self {
        assert_eq!(self.code, Some(0), "{}", self.stderr);
        self
    }

    /// Exit 1 with `message` on stderr.
    #[track_caller]
    fn refused(self, message: &str) {
        assert_eq!(self.code, Some(1), "{}", self.stderr);
        assert!(self.stderr.contains(message), "expected {message:?} in {}", self.stderr);
    }
}

fn preset() -> Value {
    serde_json::from_str(&fs::read_to_string(PRESET).unwrap()).unwrap()
}

#[test]
fn every_action_is_git_itself_or_an_action_the_script_knows() {
    let repo = Repo::new();
    let preset = preset();
    let actions = preset["actions"].as_object().unwrap();
    let options = preset["questions"]["command"]["criteria"].as_object().unwrap();
    assert!(options.keys().all(|option| actions.contains_key(option)), "`from` needs every option to be an action");
    for (name, action) in actions {
        let run: Vec<&str> = action["run"].as_array().unwrap().iter().map(|arg| arg.as_str().unwrap()).collect();
        match run.as_slice() {
            ["git", ..] => {}
            ["git/run.sh", rest @ ..] => {
                let acted = repo.act_with(rest, "", &[("JEV_GIT_DRY_RUN", "1")]);
                assert_ne!(acted.code, Some(2), "{name}: {}", acted.stderr);
            }
            other => panic!("{name}: runs {other:?}"),
        }
    }
    let unknown = repo.act(&["rebase"], "");
    assert_eq!((unknown.code, unknown.stderr.as_str()), (Some(2), "jev git: unknown action: rebase\n"));
}

#[test]
fn a_branch_is_found_among_the_words_of_the_request() {
    let repo = Repo::new();
    repo.act(&["switch"], "switch to feature/login, please.").ok();
    assert_eq!(repo.branch(), "feature/login");
    let back = repo.act(&["switch"], "go back to main").ok();
    assert!(back.stderr.starts_with("+ git switch main\n"), "{}", back.stderr);
    assert_eq!(repo.branch(), "main");

    repo.act(&["switch"], "switch branches").refused("the request names no branch");
    repo.act(&["switch"], "switch to main").refused("already on main");
    repo.act(&["switch"], "switch from main to feature/login").refused("more than one branch");
    assert_eq!(repo.branch(), "main");
    repo.act(&["delete_branch"], "delete fix-typo and feature/login")
        .refused("the request names more than one branch: fix-typo feature/login\n");
    repo.act(&["delete_branch"], "delete main and fix-typo").refused("more than one branch");
    assert!(repo.has_branch("fix-typo"));
    repo.act(&["merge"], "merge main and feature/login").refused("more than one branch");

    repo.act(&["delete_branch"], "delete the merged branch fix-typo").ok();
    assert!(!repo.has_branch("fix-typo"));
}

#[test]
fn a_new_branch_is_named_in_quotes_after_called_or_as_the_one_new_looking_word() {
    let repo = Repo::new();
    repo.act(&["create_branch"], r#"start a branch "feat/api-v2""#).ok();
    assert_eq!(repo.branch(), "feat/api-v2");
    repo.act(&["create_branch"], "create a branch called login from feature/login").ok();
    assert_eq!(repo.branch(), "login");
    repo.act(&["create_branch"], "new branch fix-42 off main").ok();
    assert_eq!(repo.branch(), "fix-42");
    assert_eq!(repo.git(&["rev-parse", "fix-42"]), repo.git(&["rev-parse", "main"]));

    repo.act(&["create_branch"], "make a branch for the payments work").refused("put the new branch's name in quotes");
    repo.act(&["create_branch"], "a branch for fix-1 or fix-2").refused("put the new branch's name in quotes");
}

#[test]
fn a_commit_takes_its_message_from_quotes_and_never_guesses_one() {
    let repo = Repo::new();
    repo.write("a.txt", "a\nchanged\n");
    repo.write("new.txt", "new\n");
    repo.act(&["commit", "--all"], "commit everything").refused("put the commit message in quotes");
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1", "nothing was committed");

    repo.act(&["commit"], "commit a.txt as 'Change a', it's ready").ok();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Change a");
    assert_eq!(repo.git(&["show", "--name-only", "--format=", "HEAD"]), "a.txt", "only the named file");

    repo.act(&["commit", "--all"], "commit all my work as “Add new”").ok();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Add new");
    assert_eq!(repo.git(&["status", "--porcelain"]), "", "--all took the untracked file too");
    repo.write("a.txt", "a\nagain\n");
    repo.write("new.txt", "new\nagain\n");
    repo.act(&["commit", "--all"], "commit all including a.txt as 'Both files'").ok();
    assert_eq!(repo.git(&["show", "--name-only", "--format=", "HEAD"]), "a.txt\nnew.txt");
}

#[test]
fn stage_and_unstage_name_files_that_exist() {
    let repo = Repo::new();
    repo.write("a.txt", "a\nchanged\n");
    repo.write("src/b.rs", "b\nchanged\n");
    repo.act(&["stage"], "stage the changes").refused("the request names no file to stage");
    repo.act(&["stage"], "stage a.txt, not the rest").ok();
    assert_eq!(repo.git(&["diff", "--staged", "--name-only"]), "a.txt");
    repo.act(&["unstage"], "unstage everything").ok();
    assert_eq!(repo.git(&["diff", "--staged", "--name-only"]), "");
    repo.act(&["stage", "--all"], "stage everything").ok();
    assert_eq!(repo.git(&["diff", "--staged", "--name-only"]), "a.txt\nsrc/b.rs");
    let diff = repo.act(&["diff", "--staged"], "what is staged in src/b.rs?").ok();
    assert!(diff.stderr.starts_with("+ git diff --staged -- src/b.rs\n"), "{}", diff.stderr);
    assert!(diff.stdout.contains("+changed"), "{}", diff.stdout);
}

#[test]
fn a_stash_keeps_its_quoted_message_and_a_named_entry_comes_back() {
    let repo = Repo::new();
    repo.write("a.txt", "a\nfirst\n");
    repo.act(&["stash"], "stash this as 'first try'").ok();
    repo.write("untracked.txt", "u\n");
    repo.act(&["stash", "--all"], "put everything aside, new files too").ok();
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    assert!(repo.git(&["stash", "list"]).contains("stash@{1}: On main: first try"));
    repo.act(&["unstash"], "bring back stash@{1}").ok();
    assert_eq!(fs::read_to_string(repo.path.join("a.txt")).unwrap(), "a\nfirst\n");
    assert!(!repo.path.join("untracked.txt").exists(), "stash@{{0}} still has it");
    assert!(repo.git(&["stash", "list"]).starts_with("stash@{0}: WIP on main"));
}

#[test]
fn a_first_push_sets_the_upstream() {
    let repo = Repo::new();
    repo.git(&["switch", "--quiet", "feature/login"]);
    let first = repo.act(&["push"], "push this branch").ok();
    assert!(first.stderr.starts_with("+ git push --set-upstream origin feature/login\n"), "{}", first.stderr);
    let again = repo.act(&["push"], "push").ok();
    assert!(again.stderr.starts_with("+ git push\n"), "{}", again.stderr);
    repo.git(&["switch", "--quiet", "--detach"]);
    repo.act(&["push"], "push").refused("HEAD is detached");
}

#[test]
fn compare_and_log_read_branches_counts_and_paths() {
    let repo = Repo::new();
    repo.git(&["switch", "--quiet", "feature/login"]);
    repo.write("login.rs", "login\n");
    repo.git(&["add", "login.rs"]);
    repo.git(&["commit", "--quiet", "--message", "Add login"]);
    let compared = repo.act(&["compare"], "what is on this branch that is not on main?").ok();
    assert!(compared.stdout.starts_with("> "), "{}", compared.stdout);
    assert!(compared.stdout.contains("Add login") && compared.stdout.contains("login.rs"), "{}", compared.stdout);
    let default = repo.act(&["compare"], "how far is this branch from where it started").ok();
    assert!(
        default.stderr.starts_with("+ git log --oneline --decorate --left-right main...HEAD\n"),
        "{}",
        default.stderr
    );

    let log = repo.act(&["log"], "show the last 1 commit of main touching a.txt").ok();
    assert!(log.stderr.starts_with("+ git log --oneline --decorate -n 1 main -- a.txt\n"), "{}", log.stderr);
    assert!(log.stdout.ends_with(" init\n"), "{}", log.stdout);
}

#[test]
fn worktrees_are_added_found_and_removed_by_name() {
    let repo = Repo::new();
    let sibling = repo.parent().join("repo-feature-login");
    let added = repo.act(&["worktree_add"], "open a worktree for feature/login").ok();
    assert_eq!(added.stdout, format!("{}\n", sibling.display()), "stdout is only the path: {}", added.stderr);
    assert!(sibling.join("a.txt").is_file());

    let root = tempfile::tempdir().unwrap();
    let under = root.path().display().to_string();
    let new = repo
        .act_with(&["worktree_add"], "new worktree for fix/redirect from main", &[("JEV_GIT_WORKTREES", &under)])
        .ok();
    assert_eq!(new.stdout, format!("{}\n", root.path().join("fix-redirect").display()));
    assert_eq!(repo.git(&["rev-parse", "fix/redirect"]), repo.git(&["rev-parse", "main"]));

    repo.act(&["worktree_add"], "a worktree for main").refused("main is already checked out");
    repo.act(&["worktree_add"], "a worktree for the payments refactor").refused("the request names no branch");

    let found = repo.act(&["worktree_path"], "where's the login worktree?").ok();
    assert_eq!(found.stdout, format!("{}\n", sibling.display()));
    repo.act(&["worktree_path"], "where is the worktree").refused("the request names no worktree");

    repo.act(&["worktree_remove"], "remove the worktree for main").refused("that is the main worktree");
    repo.act(&["worktree_remove"], "remove the worktrees for main and feature/login")
        .refused("more than one worktree");
    assert!(sibling.exists());
    let removed = repo.act(&["worktree_remove"], "remove the login worktree, it is merged").ok();
    assert!(removed.stderr.contains("the branch feature/login is kept"), "{}", removed.stderr);
    assert!(!sibling.exists());
    assert!(repo.has_branch("feature/login"));
}

#[test]
fn a_dirty_worktree_is_not_removed() {
    let repo = Repo::new();
    let path = repo.act(&["worktree_add"], "worktree for 'wip' at ../wip").ok().stdout;
    fs::write(repo.path.join(path.trim_end()).join("a.txt"), "dirty\n").unwrap();
    let acted = repo.act(&["worktree_remove"], "remove the wip worktree");
    assert_eq!(acted.code, Some(128), "git's own refusal: {}", acted.stderr);
    assert!(repo.parent().join("wip").join("a.txt").is_file());
}

#[test]
fn guards_and_a_dry_run_change_nothing() {
    let repo = Repo::new();
    repo.act(&["refuse"], "throw away all my changes").refused("does not discard work");
    repo.act(&["unsupported"], "rebase onto main").refused("not one of the operations");
    repo.act(&["unsure"], "hmm").refused("nothing ran");
    assert!(repo.act(&["help"], "what can you do?").ok().stdout.contains("Worktrees"));

    let dry = repo.act_with(&["switch"], "switch to fix-typo", &[("JEV_GIT_DRY_RUN", "1")]).ok();
    assert_eq!(dry.stderr, "+ git switch fix-typo\n");
    assert_eq!(repo.branch(), "main");

    let elsewhere = tempfile::tempdir().unwrap();
    let mut command = isolated(Command::new(SCRIPT));
    command.current_dir(elsewhere.path()).arg("switch");
    Acted::from(feed(command, "switch to main")).refused("not inside a git repository");
}

/// Answers for the preset's four questions, with `command` chosen at `confidence`.
fn answers(command: &str, confidence: f64, all_changes: f64) -> String {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "command": {
                "type": "choice",
                "choice": command,
                "confidence": confidence,
                "probabilities": { "status": 1.0 - confidence, (command): confidence }
            },
            "destructive": { "type": "noul", "noul": 0.02 },
            "all_changes": { "type": "noul", "noul": all_changes },
            "staged": { "type": "noul", "noul": 0.05 }
        },
        "usage": { "input_tokens": 700, "output_tokens": 40 }
    })
    .to_string()
}

fn jev(repo: &Repo, server: &common::Server, request: &str) -> Acted {
    let base_url = format!("http://127.0.0.1:{}", server.port);
    let mut command = isolated(Command::new(env!("CARGO_BIN_EXE_jev")));
    command.current_dir(&repo.path).args(["call", PRESET, "-s", request, "--api-key", "test", "--retries", "0"]);
    command.args(["--timeout", "5", "--base-url", &base_url]).stdin(Stdio::null());
    Acted::from(command.output().unwrap())
}

#[test]
fn jev_call_runs_the_chosen_action_with_the_request_on_stdin() {
    let repo = Repo::new();
    let server = common::spawn_all(&[(200, &answers("worktree_add", 0.91, 0.1)), (200, &answers("commit", 0.88, 0.8))]);

    let added = jev(&repo, &server, "spin up a worktree for fix/login from main").ok();
    assert_eq!(added.stdout, format!("{}\n", repo.parent().join("repo-fix-login").display()), "{}", added.stderr);
    assert!(server.request().contains("spin up a worktree"));

    repo.write("new.txt", "new\n");
    jev(&repo, &server, "commit all of it as 'Add new'").ok();
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Add new", "commit_all staged the untracked file");
}

#[test]
fn a_change_below_the_confidence_bar_runs_the_fallback() {
    let repo = Repo::new();
    repo.git(&["commit", "--quiet", "--allow-empty", "--message", "unpushed"]);
    let server = common::spawn(200, &answers("push", 0.55, 0.1));
    jev(&repo, &server, "push?").refused("not sure what that asks for");
    assert_eq!(repo.git(&["rev-list", "--count", "origin/main..main"]), "1", "nothing was pushed");
}
