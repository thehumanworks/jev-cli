//! Binary tests. Help text is pinned to fixtures. HTTP tests talk to a local fake server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::fs;
use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;

fn jev() -> Command {
    let mut command = Command::cargo_bin("jev").unwrap();
    command.env_clear().timeout(Duration::from_secs(10));
    command
}

#[test]
fn help_fixtures_match_exactly() {
    let cases = [
        (vec!["--help"], "jev.txt"),
        (vec!["ask", "--help"], "ask.txt"),
        (vec!["noul", "--help"], "noul.txt"),
        (vec!["choice", "--help"], "choice.txt"),
        (vec!["score", "--help"], "score.txt"),
        (vec!["decide", "--help"], "decide.txt"),
        (vec!["call", "--help"], "call.txt"),
        (vec!["example", "--help"], "example.txt"),
        (vec!["completions", "--help"], "completions.txt"),
    ];
    for (args, fixture) in cases {
        let expected = fs::read_to_string(format!("tests/fixtures/help/{fixture}")).unwrap();
        jev().args(&args).assert().success().stdout(expected.clone()).stderr("");
        jev().env("COLUMNS", "40").args(args).assert().success().stdout(expected);
    }
}

#[test]
fn version_is_the_package_version() {
    let expected = format!("jev {}\n", env!("CARGO_PKG_VERSION"));
    jev().arg("--version").assert().success().stdout(expected);
}

#[test]
fn example_pipes_into_ask_dry_run() {
    let example = jev().arg("example").assert().success().get_output().stdout.clone();
    jev()
        .args(["ask", "-q", "-", "-s", "Please help ASAP", "--dry-run"])
        .write_stdin(example)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"is_urgent\""))
        .stdout(predicate::str::contains("\"department\""));
}

#[test]
fn completions_bash_is_non_empty() {
    jev()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not())
        .stdout(predicate::str::contains("jev"));
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    let output = jev().arg("--not-a-flag").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("jev: "), "{stderr}");
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.ends_with('\n'));
}

#[test]
fn http_statuses_map_to_exit_codes_and_send_auth() {
    let ok = common::spawn(
        200,
        r#"{"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.95}},"usage":{"input_tokens":318,"output_tokens":34}}"#,
    );
    let output = jev()
        .args([
            "noul",
            "Is it spam?",
            "-s",
            "buy now",
            "--json",
            "--api-key",
            "test",
            "--retries",
            "0",
            "--timeout",
            "5",
            "--base-url",
            &format!("http://127.0.0.1:{}", ok.port),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert!(value["cost_usd"].is_number(), "{stdout}");
    assert_eq!(value["model"], "jev-1.13.0");
    assert_eq!(value["answers"]["q"]["type"], "noul");
    let raw = ok.request();
    assert_headers(&raw);
    assert!(raw.contains("Is it spam?"), "{raw}");

    for (status, body, code) in [(401, "unauthorized", 3), (422, "bad", 4), (413, "too big", 5), (500, "boom", 6)] {
        let server = common::spawn(status, body);
        let asserted = jev()
            .args([
                "noul",
                "Is it spam?",
                "-s",
                "buy now",
                "--api-key",
                "test",
                "--retries",
                "0",
                "--timeout",
                "5",
                "--base-url",
                &format!("http://127.0.0.1:{}", server.port),
            ])
            .assert()
            .code(code);
        let stderr = String::from_utf8_lossy(&asserted.get_output().stderr);
        assert!(stderr.starts_with("jev: "), "{status}: {stderr}");
        assert!(!stderr.contains("Bearer test"), "{stderr}");
        let raw = server.request();
        assert_headers(&raw);
    }
}

/// Header names are matched without case. ureq sends them in lowercase.
fn assert_headers(raw: &str) {
    let lower = raw.to_ascii_lowercase();
    assert!(lower.contains("authorization: bearer test"), "{raw}");
    assert!(lower.contains("content-type: application/json"), "{raw}");
}

#[test]
fn process_environment_supplies_the_key_the_model_and_the_base_url() {
    let server = common::spawn(
        200,
        r#"{"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.5}},"usage":{"input_tokens":1,"output_tokens":1}}"#,
    );
    jev()
        .env("TYPESAFE_API_KEY", "test")
        .env("JEV_BASE_URL", format!("http://127.0.0.1:{}", server.port))
        .env("JEV_MODEL", "from-env")
        .args(["noul", "Is it spam?", "-s", "buy now", "--retries", "0", "--timeout", "5"])
        .assert()
        .success();
    let raw = server.request();
    assert_headers(&raw);
    assert!(raw.contains("\"from-env\""), "{raw}");
}

#[test]
fn a_retried_503_prints_the_attempt_before_the_verbose_line() {
    let server = common::spawn_all(&[
        (503, "busy"),
        (
            200,
            r#"{"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.5}},"usage":{"input_tokens":1,"output_tokens":1}}"#,
        ),
    ]);
    let output = jev()
        .args([
            "noul",
            "Is it spam?",
            "-s",
            "buy now",
            "--api-key",
            "test",
            "--retries",
            "1",
            "--debug",
            "-v",
            "--timeout",
            "5",
            "--base-url",
            &format!("http://127.0.0.1:{}", server.port),
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let attempt = stderr.find("attempt 1:").unwrap_or_else(|| panic!("{stderr}"));
    let verbose = stderr.find("retries").unwrap_or_else(|| panic!("{stderr}"));
    assert!(attempt < verbose, "{stderr}");
}

#[test]
fn process_environment_timeout_must_be_a_number() {
    let output = jev().env("JEV_TIMEOUT", "nope").args(["noul", "q", "-s", "hi", "--dry-run"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("JEV_TIMEOUT"), "{stderr}");
}

#[test]
fn missing_api_key_exits_3_even_if_the_parent_would_have_one() {
    let output = jev().args(["noul", "Is it spam?", "-s", "hello", "--retries", "0"]).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr, "jev: no API key; pass --api-key or set TYPESAFE_API_KEY\n");
}

/// The answers `jev example`'s questions get in the README.
const ASK: &str = r#"{"model":"jev-1.13.0","answers":{"is_urgent":{"type":"noul","noul":0.95},"department":{"type":"choice","choice":"technical","confidence":0.78,"probabilities":{"billing":0.15,"technical":0.85,"sales":0.0}},"frustration":{"type":"score","score":1.0,"confidence":0.7,"legend":{"0":"a","1":"b","2":"c"},"probabilities":{"0":0.1,"1":0.8,"2":0.1}}},"usage":{"input_tokens":318,"output_tokens":34}}"#;

/// A flaky CI failure for `tests/fixtures/presets/ci.json`: it decides `retry`.
const FLAKY: &str = r#"{"model":"jev-1.13.0","answers":{"secrets":{"type":"noul","noul":0.03},"kind":{"type":"choice","choice":"retry","confidence":0.84,"probabilities":{"retry":0.9,"fmt":0.02,"issue":0.08}},"scope":{"type":"score","score":0.3,"confidence":0.7,"legend":{"0":"a","1":"b","2":"c"},"probabilities":{"0":0.75,"1":0.2,"2":0.05}}},"usage":{"input_tokens":900,"output_tokens":60}}"#;

#[test]
fn a_named_preset_is_found_from_a_subdirectory_and_its_action_can_call_another() {
    let project = tempfile::tempdir().unwrap();
    let presets = project.path().join(".jev").join("presets");
    fs::create_dir_all(&presets).unwrap();
    let work = project.path().join("src").join("deep");
    fs::create_dir_all(&work).unwrap();
    let server = common::spawn_all(&[(200, ASK), (200, FLAKY)]);
    let base_url = format!("http://127.0.0.1:{}", server.port);

    // The outer preset's `technical` action is `jev decide inner`: it reads the state from its
    // stdin, which is the state the outer `call` was given.
    let example = jev().args(["example", "--preset"]).assert().success().get_output().stdout.clone();
    let mut outer: Value = serde_json::from_slice(&example).unwrap();
    let inner_call =
        [env!("CARGO_BIN_EXE_jev"), "decide", "inner", "--api-key", "test", "--retries", "0", "--base-url", &base_url];
    outer["actions"]["technical"]["run"] = serde_json::json!(inner_call);
    fs::write(presets.join("support.json"), outer.to_string()).unwrap();
    fs::copy("tests/fixtures/presets/ci.json", presets.join("inner.json")).unwrap();

    let state = "UNIQUE-STATE the Stripe integration fails, please help";
    let output = jev()
        .current_dir(&work)
        .args(["call", "support", "-s", state, "--api-key", "test", "--retries", "0", "--timeout", "5"])
        .args(["--base-url", &base_url])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "retry\n", "{stderr}");
    let outer_request = server.request();
    assert!(outer_request.contains("\"department\""), "{outer_request}");
    let inner_request = server.request();
    assert!(inner_request.contains("\"secrets\""), "{inner_request}");
    assert!(inner_request.contains("UNIQUE-STATE"), "the action got the state on stdin: {inner_request}");
}

#[test]
fn a_named_preset_falls_back_to_the_config_directory() {
    let config = tempfile::tempdir().unwrap();
    let presets = config.path().join("jev").join("presets");
    fs::create_dir_all(&presets).unwrap();
    fs::copy("tests/fixtures/presets/ci.json", presets.join("ci.json")).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    jev()
        .current_dir(elsewhere.path())
        .env("XDG_CONFIG_HOME", config.path())
        .args(["decide", "ci", "-s", "a log", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"secrets\""));
    jev()
        .current_dir(elsewhere.path())
        .env("HOME", elsewhere.path())
        .args(["decide", "ci", "-s", "a log", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicate::str::starts_with("jev: no preset named `ci` in .jev/presets or "));
}
