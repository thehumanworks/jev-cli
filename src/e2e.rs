//! In-process command tests. The transport is fake, and the environment is the map on [`Io`],
//! never the developer's process environment.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Cursor, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use typesafe_jev::{Reply, Transport};

use crate::questions::{EMPTY_QUESTIONS, MISSING_QUESTIONS};
use crate::state::{EMPTY_STATE, EXAMPLE_STATE, TERMINAL_STATE};
use crate::{Io, run};

const NOUL: &str = r#"{"model":"jev-1.13.0","answers":{"q":{"type":"noul","noul":0.95}},"usage":{"input_tokens":318,"output_tokens":34}}"#;

const CHOICE: &str = r#"{
    "model": "jev-1.13.0",
    "answers": {
        "q": {
            "type": "choice",
            "choice": "technical",
            "confidence": 0.78,
            "probabilities": {"billing": 0.15, "technical": 0.85, "sales": 0.0}
        }
    },
    "usage": {"input_tokens": 10, "output_tokens": 2}
}"#;

const SCORE: &str = r#"{
    "model": "jev-1.13.0",
    "answers": {
        "q": {
            "type": "score",
            "score": 1.25,
            "confidence": 0.5,
            "legend": {"0": "low", "1": "high"},
            "probabilities": {"0": 0.25, "1": 0.75}
        }
    },
    "usage": {"input_tokens": 10, "output_tokens": 2}
}"#;

const ASK: &str = r#"{
    "model": "jev-1.13.0",
    "answers": {
        "is_urgent": {"type": "noul", "noul": 0.95},
        "department": {
            "type": "choice",
            "choice": "technical",
            "confidence": 0.78,
            "probabilities": {"billing": 0.15, "technical": 0.85, "sales": 0.0}
        },
        "frustration": {
            "type": "score",
            "score": 1.0,
            "confidence": 0.7,
            "legend": {
                "0": "Calm, just stating facts",
                "1": "Frustrated but civil",
                "2": "Very angry, strong language"
            },
            "probabilities": {"0": 0.1, "1": 0.8, "2": 0.1}
        }
    },
    "usage": {"input_tokens": 318, "output_tokens": 34}
}"#;

const ASK_TEXT: &str = "\
department: technical (confidence 0.78)
  billing: 0.15
  technical: 0.85
  sales: 0.00
frustration: 1.00 (confidence 0.70)
  0 Calm, just stating facts: 0.10
  1 Frustrated but civil: 0.80
  2 Very angry, strong language: 0.10
is_urgent: 0.95
";

struct Run {
    args: Vec<OsString>,
    stdin: Vec<u8>,
    terminal: bool,
    env: BTreeMap<String, String>,
    transport: Option<Box<dyn Transport>>,
}

struct Out {
    code: u8,
    stdout: String,
    stderr: String,
}

impl Run {
    fn new(args: &[&str]) -> Self {
        let mut full = Vec::with_capacity(args.len() + 1);
        full.push(OsString::from("jev"));
        full.extend(args.iter().map(OsString::from));
        Self { args: full, stdin: Vec::new(), terminal: false, env: BTreeMap::new(), transport: None }
    }

    fn stdin(mut self, text: &str) -> Self {
        self.stdin = text.as_bytes().to_vec();
        self
    }

    fn terminal(mut self) -> Self {
        self.terminal = true;
        self
    }

    fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_owned(), value.to_owned());
        self
    }

    fn transport(mut self, transport: impl Transport + 'static) -> Self {
        self.transport = Some(Box::new(transport));
        self
    }

    fn go(self) -> Out {
        let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut io = Io {
            args: self.args,
            stdin: Cursor::new(self.stdin),
            stdout: Vec::new(),
            stderr: Arc::clone(&stderr),
            stdin_is_terminal: self.terminal,
            env: Some(self.env),
        };
        let code = run(&mut io, self.transport);
        let stderr = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
        Out { code, stdout: String::from_utf8(io.stdout).unwrap(), stderr }
    }
}

fn ok_body(body: &'static str) -> impl Transport {
    move |_request: &[u8]| Ok(Reply { status: 200, retry_after: None, body: body.to_owned() })
}

fn status_body(status: u16, body: impl Into<String>) -> impl Transport {
    let body = body.into();
    move |_request: &[u8]| Ok(Reply { status, retry_after: None, body: body.clone() })
}

fn refuse() -> impl Transport {
    |_request: &[u8]| Err("transport should not be called".into())
}

#[test]
fn noul_choice_and_score_text_are_bare_values() {
    let noul = Run::new(&["noul", "Is it spam?", "-s", "buy now", "--api-key", "test", "--retries", "0"])
        .transport(ok_body(NOUL))
        .go();
    assert_eq!(noul.code, 0);
    assert_eq!(noul.stdout, "0.95\n");
    assert_eq!(noul.stderr, "");

    let choice = Run::new(&[
        "choice",
        "Which team?",
        "billing=Payments",
        "technical=Bugs",
        "sales=Pricing",
        "-s",
        "the integration fails",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .transport(ok_body(CHOICE))
    .go();
    assert_eq!(choice.stdout, "technical\n");
    assert_eq!(choice.stderr, "");

    let score =
        Run::new(&["score", "How urgent?", "low", "high", "-s", "please help", "--api-key", "test", "--retries", "0"])
            .transport(ok_body(SCORE))
            .go();
    assert_eq!(score.stdout, "1.25\n");
    assert_eq!(score.code, 0);
}

#[test]
fn json_output_is_the_response_plus_cost() {
    let out = Run::new(&["noul", "Is it spam?", "-s", "buy now", "--json", "--api-key", "test", "--retries", "0"])
        .transport(ok_body(NOUL))
        .go();
    assert_eq!(out.code, 0);
    assert!(out.stdout.ends_with('\n'));
    let value: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(value["model"], "jev-1.13.0");
    assert_eq!(value["answers"]["q"]["type"], "noul");
    assert!(value["cost_usd"].is_number());
    let model = out.stdout.find("\"model\"").unwrap();
    let answers = out.stdout.find("\"answers\"").unwrap();
    let usage = out.stdout.find("\"usage\"").unwrap();
    let cost = out.stdout.find("\"cost_usd\"").unwrap();
    assert!(model < answers && answers < usage && usage < cost, "{}", out.stdout);
}

#[test]
fn ask_text_and_json_follow_the_contract() {
    let questions = include_questions();
    let text = Run::new(&["ask", "-q", "-", "-s", "Please help ASAP", "--api-key", "test", "--retries", "0"])
        .stdin(&questions)
        .transport(ok_body(ASK))
        .go();
    assert_eq!(text.code, 0, "{}", text.stderr);
    assert_eq!(text.stdout, ASK_TEXT);

    let json = Run::new(&[
        "ask",
        "-q",
        "-",
        "-s",
        "Please help ASAP",
        "--output",
        "json",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .stdin(&questions)
    .transport(ok_body(ASK))
    .go();
    assert_eq!(json.code, 0, "{}", json.stderr);
    assert!(json.stdout.contains("\"cost_usd\""));
    let urgent = json.stdout.find("\"is_urgent\"").unwrap();
    let department = json.stdout.find("\"department\"").unwrap();
    assert!(urgent < department, "json keeps API answer order\n{}", json.stdout);
}

#[test]
fn dry_run_matches_the_body_the_client_would_send() {
    let args = [
        "noul",
        "Is it spam?",
        "--yes",
        "spam",
        "--no",
        "ham",
        "-s",
        "hello",
        "--model",
        "jev-latest",
        "--api-key",
        "test",
        "--retries",
        "0",
    ];
    let dry = Run::new(&args).arg_tail(&["--dry-run"]).transport(refuse()).go();
    assert_eq!(dry.code, 0, "{}", dry.stderr);
    assert_eq!(dry.stderr, "");
    assert!(dry.stdout.ends_with('\n'));

    let seen = Arc::new(Mutex::new(Vec::new()));
    let slot = Arc::clone(&seen);
    let sent = Run::new(&args)
        .transport(move |request: &[u8]| {
            slot.lock().unwrap().extend_from_slice(request);
            Ok(Reply { status: 200, retry_after: None, body: NOUL.to_owned() })
        })
        .go();
    assert_eq!(sent.code, 0, "{}", sent.stderr);
    let wire = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
    assert!(wire.contains(r#""criteria":{"true":"spam","false":"ham"}"#), "{wire}");
    let dry_value: Value = serde_json::from_str(&dry.stdout).unwrap();
    let wire_value: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(dry_value, wire_value);
    let model = wire.find("\"model\"").unwrap();
    let state = wire.find("\"state\"").unwrap();
    let questions = wire.find("\"questions\"").unwrap();
    assert!(model < state && state < questions, "{wire}");
}

#[test]
fn dry_run_needs_no_key_and_keeps_json_state_order() {
    let out = Run::new(&["noul", "Is it empty?", "-s", r#"{"z":1,"a":2}"#, "--state-json", "--dry-run"])
        .transport(refuse())
        .go();
    assert_eq!(out.code, 0, "{}", out.stderr);
    let zed = out.stdout.find("\"z\"").unwrap();
    let ay = out.stdout.find("\"a\"").unwrap();
    assert!(zed < ay, "{}", out.stdout);
    assert!(out.stdout.contains("\"state\": {"));
}

#[test]
fn threshold_branches() {
    let met = Run::new(&[
        "noul",
        "Is it spam?",
        "-s",
        "buy now",
        "--threshold",
        "0.8",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .transport(ok_body(NOUL))
    .go();
    assert_eq!(met.code, 0);
    assert_eq!(met.stdout, "0.95\n");

    let missed = Run::new(&[
        "noul",
        "Is it spam?",
        "-s",
        "buy now",
        "--threshold",
        "0.99",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .transport(ok_body(NOUL))
    .go();
    assert_eq!(missed.code, 1);
    assert_eq!(missed.stdout, "0.95\n");
    assert_eq!(missed.stderr, "");

    let equal = Run::new(&[
        "noul",
        "Is it spam?",
        "-s",
        "buy now",
        "--threshold",
        "0.5",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .transport(ok_body(
        r#"{"model":"m","answers":{"q":{"type":"noul","noul":0.5}},"usage":{"input_tokens":1,"output_tokens":1}}"#,
    ))
    .go();
    assert_eq!(equal.code, 0, "{}", equal.stderr);
    assert_eq!(equal.stdout, "0.5\n");

    let bad = Run::new(&["noul", "Is it spam?", "-s", "buy now", "--threshold", "2", "--api-key", "test"])
        .transport(refuse())
        .go();
    assert_eq!(bad.code, 2);
    assert!(bad.stderr.contains("between 0 and 1"), "{}", bad.stderr);
}

#[test]
fn verbose_and_debug_lines() {
    let out = Run::new(&["noul", "Is it spam?", "-s", "buy now", "-v", "--api-key", "test", "--retries", "0"])
        .transport(ok_body(NOUL))
        .go();
    assert_eq!(out.stderr, "jev-1.13.0: 318 input tokens, 34 output tokens, 0 retries, ~$0.000013\n");

    let missed = Run::new(&[
        "noul",
        "Is it spam?",
        "-s",
        "buy now",
        "-v",
        "--threshold",
        "0.99",
        "--api-key",
        "test",
        "--retries",
        "0",
    ])
    .transport(ok_body(NOUL))
    .go();
    assert_eq!(missed.code, 1);
    assert!(missed.stderr.contains("0 retries"), "{}", missed.stderr);

    let calls = AtomicUsize::new(0);
    let debug =
        Run::new(&["noul", "Is it spam?", "-s", "buy now", "--debug", "-v", "--api-key", "test", "--retries", "1"])
            .transport(move |_request: &[u8]| {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(Reply { status: 500, retry_after: None, body: "busy".into() })
                } else {
                    Ok(Reply { status: 200, retry_after: None, body: NOUL.to_owned() })
                }
            })
            .go();
    assert_eq!(debug.code, 0, "{}", debug.stderr);
    assert!(debug.stderr.contains("attempt 1:"), "{}", debug.stderr);
    assert!(debug.stderr.contains("HTTP 500"), "{}", debug.stderr);
    assert!(debug.stderr.contains("1 retries"), "{}", debug.stderr);
    assert_eq!(debug.stdout, "0.95\n");
}

#[test]
fn each_error_variant_has_its_exit_code() {
    let base = ["noul", "Is it spam?", "-s", "hello", "--api-key", "test", "--retries", "0"];
    let cases = [(401, "nope", 3), (422, "bad request", 4), (413, "too big", 5), (500, "boom", 6), (400, "no", 6)];
    for (status, body, code) in cases {
        let out = Run::new(&base).transport(status_body(status, body)).go();
        assert_eq!(out.code, code, "status {status}: {}", out.stderr);
        assert!(out.stderr.starts_with("jev: "), "{}", out.stderr);
        assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
        assert!(!out.stderr.contains(body), "status {status} forwarded the body: {}", out.stderr);
    }

    let missing = Run::new(&["noul", "Is it spam?", "-s", "hello"]).transport(refuse()).go();
    assert_eq!(missing.code, 3);
    assert!(missing.stderr.contains("TYPESAFE_API_KEY"), "{}", missing.stderr);

    let config = Run::new(&["noul", "Is it spam?", "-s", "hello", "--api-key", "test", "--base-url", "not a url"]).go();
    assert_eq!(config.code, 4, "{}", config.stderr);
    assert!(!config.stderr.contains("hello"), "{}", config.stderr);

    let reset = Run::new(&base).transport(|_request: &[u8]| Err("connection reset".into())).go();
    assert_eq!(reset.code, 6, "{}", reset.stderr);
    assert!(reset.stderr.contains("connection reset"), "{}", reset.stderr);

    let io_err = Run::new(&["noul", "Is it spam?", "-f", "/no/such/jev/state", "--api-key", "test"]).go();
    assert_eq!(io_err.code, 7, "{}", io_err.stderr);
    assert!(io_err.stderr.contains("cannot read state"), "{}", io_err.stderr);
}

#[test]
fn a_long_echoed_state_stays_off_stderr_unless_debug_is_set() {
    let state = "UNIQUE_STATE_PREFIX_".repeat(50);
    assert_eq!(state.len(), 1000);
    let body = format!("echo {state}");
    let hidden = Run::new(&["noul", "Is it spam?", "-s", &state, "--api-key", "test", "--retries", "0"])
        .transport(status_body(422, body.clone()))
        .go();
    assert_eq!(hidden.code, 4, "{}", hidden.stderr);
    assert!(!hidden.stderr.contains(&state), "{}", hidden.stderr);
    assert!(!hidden.stderr.contains("UNIQUE_STATE_PREFIX_"), "{}", hidden.stderr);
    assert!(hidden.stderr.contains("HTTP 422"), "{}", hidden.stderr);
    assert!(hidden.stderr.contains("re-run with --debug"), "{}", hidden.stderr);

    let shown = Run::new(&["noul", "Is it spam?", "-s", &state, "--api-key", "test", "--retries", "0", "--debug"])
        .transport(status_body(422, body))
        .go();
    assert_eq!(shown.code, 4, "{}", shown.stderr);
    assert!(shown.stderr.contains("UNIQUE_STATE_PREFIX_"), "{}", shown.stderr);
}

#[test]
fn debug_lines_are_on_stderr_before_the_next_attempt() {
    let stderr = Arc::new(Mutex::new(Vec::<u8>::new()));
    let seen = Arc::clone(&stderr);
    let calls = AtomicUsize::new(0);
    let mut io = Io {
        args: ["jev", "noul", "Is it spam?", "-s", "buy now", "--debug", "-v", "--api-key", "test", "--retries", "1"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        stdin: Cursor::new(Vec::<u8>::new()),
        stdout: Vec::new(),
        stderr: Arc::clone(&stderr),
        stdin_is_terminal: false,
        env: Some(BTreeMap::new()),
    };
    let code = run(
        &mut io,
        Some(Box::new(move |_request: &[u8]| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Ok(Reply { status: 500, retry_after: None, body: "busy".into() });
            }
            let text = String::from_utf8(seen.lock().unwrap().clone()).unwrap();
            assert!(text.contains("attempt 1:"), "debug must already be written, saw {text:?}");
            Ok(Reply { status: 200, retry_after: None, body: NOUL.to_owned() })
        })),
    );
    assert_eq!(code, 0);
    let text = String::from_utf8(stderr.lock().unwrap().clone()).unwrap();
    let debug_at = text.find("attempt 1:").unwrap();
    let verbose_at = text.find("retries").unwrap();
    assert!(debug_at < verbose_at, "{text}");
}

#[test]
fn arguments_are_checked_before_stdin_is_read() {
    let choice = Run::new(&["choice", "Q", "only"]).terminal().go();
    assert_eq!(choice.code, 2);
    assert!(choice.stderr.contains("between 2 and 255"), "{}", choice.stderr);
    assert!(!choice.stderr.contains("terminal"), "{}", choice.stderr);

    let score = Run::new(&["score", "Q", "only"]).terminal().go();
    assert_eq!(score.code, 2);
    assert!(score.stderr.contains("between 2 and 10"), "{}", score.stderr);
    assert!(!score.stderr.contains("terminal"), "{}", score.stderr);

    let threshold = Run::new(&["noul", "Q", "--threshold", "2"]).terminal().go();
    assert_eq!(threshold.code, 2);
    assert!(threshold.stderr.contains("between 0 and 1"), "{}", threshold.stderr);
    assert!(!threshold.stderr.contains("terminal"), "{}", threshold.stderr);

    let ask = Run::new(&["ask", "--questions-json", "not-json"]).terminal().go();
    assert_eq!(ask.code, 2);
    assert!(ask.stderr.contains("not valid"), "{}", ask.stderr);
    assert!(!ask.stderr.contains("terminal"), "{}", ask.stderr);

    let mut io = Io {
        args: ["jev", "noul", "q", "--json", "--output", "text"].into_iter().map(OsString::from).collect(),
        stdin: ExplodingStdin,
        stdout: Vec::new(),
        stderr: Arc::new(Mutex::new(Vec::new())),
        stdin_is_terminal: false,
        env: Some(BTreeMap::new()),
    };
    let code = run(&mut io, None);
    let stderr = String::from_utf8(io.stderr.lock().unwrap().clone()).unwrap();
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("--output text"), "{stderr}");
    assert!(!stderr.contains("stdin was read"), "{stderr}");
}

#[test]
fn timeout_zero_is_rejected_from_the_flag_and_the_environment() {
    let flag = Run::new(&["noul", "q", "-s", "hi", "--timeout", "0", "--dry-run"]).go();
    assert_eq!(flag.code, 2, "{}", flag.stderr);
    assert!(flag.stderr.contains("--timeout"), "{}", flag.stderr);
    assert!(flag.stderr.contains("0 is not in 1..=86400"), "{}", flag.stderr);
    assert!(!flag.stderr.contains("18446744073709551615"), "{}", flag.stderr);

    let over = Run::new(&["noul", "q", "-s", "hi", "--timeout", "86401", "--dry-run"]).go();
    assert_eq!(over.code, 2, "{}", over.stderr);
    assert!(over.stderr.contains("86401 is not in 1..=86400"), "{}", over.stderr);

    let day = Run::new(&["noul", "q", "-s", "hi", "--timeout", "86400", "--dry-run"]).go();
    assert_eq!(day.code, 0, "{}", day.stderr);

    let env = Run::new(&["noul", "q", "-s", "hi", "--dry-run"]).env("JEV_TIMEOUT", "0").go();
    assert_eq!(env.code, 2, "{}", env.stderr);
    assert!(env.stderr.contains("0 is not in 1..=86400"), "{}", env.stderr);

    let env_over = Run::new(&["noul", "q", "-s", "hi", "--dry-run"]).env("JEV_TIMEOUT", "86401").go();
    assert_eq!(env_over.code, 2, "{}", env_over.stderr);
    assert!(env_over.stderr.contains("86401 is not in 1..=86400"), "{}", env_over.stderr);
}

#[test]
fn a_missing_positional_names_the_argument() {
    let out = Run::new(&["noul"]).go();
    assert_eq!(out.code, 2);
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    assert!(out.stderr.contains("INSTRUCTIONS"), "{}", out.stderr);
}

#[test]
fn the_readme_choice_example_runs() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let example = r#"jev choice "Which sign?" -s "the value fell" -- --down --up"#;
    assert!(readme.contains(example), "README choice example drifted");
    assert!(!readme.contains(r#"jev choice "Which sign?" -- --down --up -s"#));
    let out = Run::new(&["choice", "Which sign?", "-s", "the value fell", "--dry-run", "--", "--down", "--up"]).go();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("\"--down\""), "{}", out.stdout);
    assert!(out.stdout.contains("\"--up\""), "{}", out.stdout);
}

#[test]
fn usage_errors_for_state_questions_and_stdio() {
    let terminal = Run::new(&["noul", "Is it spam?"]).terminal().go();
    assert_eq!(terminal.code, 2);
    assert_eq!(terminal.stderr, format!("jev: {TERMINAL_STATE}\n"));

    let both = Run::new(&["ask", "-q", "-", "-f", "-"]).stdin("ignored").go();
    assert_eq!(both.code, 2);
    assert!(both.stderr.contains("both"), "{}", both.stderr);

    let empty = Run::new(&["noul", "q", "-s", "   ", "--api-key", "test"]).go();
    assert_eq!(empty.stderr, format!("jev: {EMPTY_STATE}\n"));

    let number = Run::new(&["noul", "q", "-s", "1", "--state-json", "--api-key", "test"]).go();
    assert_eq!(number.code, 2);
    assert!(number.stderr.contains("number"), "{}", number.stderr);
    assert!(!number.stderr.contains(" 1"), "{}", number.stderr);

    for raw in ["true", "null", "NOT JSON"] {
        let out = Run::new(&["noul", "q", "-s", raw, "--state-json", "--api-key", "test"]).go();
        assert_eq!(out.code, 2, "{raw}: {}", out.stderr);
    }

    let missing = Run::new(&["ask", "-s", "hello"]).go();
    assert_eq!(missing.stderr, format!("jev: {MISSING_QUESTIONS}\n"));

    let empty_doc = Run::new(&["ask", "-s", "hello", "--questions-json", "{}"]).go();
    assert_eq!(empty_doc.stderr, format!("jev: {EMPTY_QUESTIONS}\n"));

    let bad_doc = Run::new(&["ask", "-s", "hello", "--questions-json", r#"{"q":{"instructions":"x"}}"#]).go();
    assert_eq!(bad_doc.code, 2);
    assert!(bad_doc.stderr.contains("missing field"), "{}", bad_doc.stderr);

    let choice = Run::new(&["choice", "Which?", "only", "-s", "hello"]).go();
    assert_eq!(choice.code, 2);
    assert!(choice.stderr.contains("between 2 and 255"), "{}", choice.stderr);

    let score = Run::new(&["score", "Rate", "only", "-s", "hello"]).go();
    assert!(score.stderr.contains("between 2 and 10"), "{}", score.stderr);
}

#[test]
fn environment_model_and_retries_follow_flag_then_env() {
    let from_env = Run::new(&["noul", "q", "-s", "hello", "--dry-run"]).env("JEV_MODEL", "from-env").go();
    assert!(from_env.stdout.contains("from-env"), "{}", from_env.stdout);

    let flag =
        Run::new(&["noul", "q", "-s", "hello", "--model", "from-flag", "--dry-run"]).env("JEV_MODEL", "from-env").go();
    assert!(flag.stdout.contains("from-flag"), "{}", flag.stdout);
    assert!(!flag.stdout.contains("from-env"), "{}", flag.stdout);

    let hits = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&hits);
    let flagged = Run::new(&["noul", "q", "-s", "hello", "--api-key", "test", "--retries", "0"])
        .env("JEV_RETRIES", "5")
        .transport(move |_request: &[u8]| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Reply { status: 500, retry_after: None, body: "no".into() })
        })
        .go();
    assert_eq!(flagged.code, 6);
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    let hits = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&hits);
    let from_env = Run::new(&["noul", "q", "-s", "hello", "--api-key", "test"])
        .env("JEV_RETRIES", "1")
        .transport(move |_request: &[u8]| {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Reply { status: 500, retry_after: None, body: "no".into() })
        })
        .go();
    assert_eq!(from_env.code, 6, "{}", from_env.stderr);
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn example_round_trips_and_completions_are_non_empty() {
    let example = Run::new(&["example"]).go();
    assert_eq!(example.code, 0);
    assert_eq!(example.stderr, "");
    let questions: typesafe_jev::Questions = serde_json::from_str(&example.stdout).unwrap();
    assert_eq!(questions.ids().collect::<Vec<_>>(), ["department", "frustration", "is_urgent"]);

    let state = Run::new(&["example", "--state"]).go();
    assert_eq!(state.stdout, format!("{EXAMPLE_STATE}\n"));

    let piped = Run::new(&["ask", "-q", "-", "-s", "Please help ASAP", "--dry-run"]).stdin(&example.stdout).go();
    assert_eq!(piped.code, 0, "{}", piped.stderr);
    assert!(piped.stdout.contains("\"is_urgent\""), "{}", piped.stdout);
    assert!(piped.stdout.contains("\"department\""), "{}", piped.stdout);

    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = Run::new(&["completions", shell]).go();
        assert_eq!(out.code, 0, "{shell}: {}", out.stderr);
        assert!(!out.stdout.is_empty(), "{shell}");
        assert!(out.stdout.contains("jev"), "{shell}");
    }
}

#[test]
fn a_broken_pipe_exits_quietly() {
    for stdout in [Broken::Write, Broken::Flush] {
        let mut io = Io {
            args: ["jev", "noul", "q", "-s", "hello", "--api-key", "test", "--retries", "0"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            stdin: Cursor::new(Vec::<u8>::new()),
            stdout,
            stderr: Arc::new(Mutex::new(Vec::new())),
            stdin_is_terminal: false,
            env: Some(BTreeMap::new()),
        };
        let code = run(&mut io, Some(Box::new(ok_body(NOUL))));
        assert_eq!(code, 0);
        let stderr = io.stderr.lock().unwrap();
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
    }
}

#[test]
fn the_readme_embeds_the_example_document_and_state() {
    let example = Run::new(&["example"]).go();
    let state = Run::new(&["example", "--state"]).go();
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    assert!(readme.contains(example.stdout.trim_end()), "README is missing the `jev example` document");
    assert!(readme.contains(state.stdout.trim_end()), "README is missing the example state");
}

#[test]
fn help_and_unknown_flags() {
    let help = Run::new(&["--help"]).env("TYPESAFE_API_KEY", "super-secret-key").go();
    assert_eq!(help.code, 0, "{}", help.stderr);
    assert_eq!(help.stderr, "");
    assert!(help.stdout.contains("exit codes"), "{}", help.stdout);
    assert!(!help.stdout.contains("super-secret-key"), "{}", help.stdout);

    let version = Run::new(&["--version"]).go();
    assert_eq!(version.stdout, format!("jev {}\n", env!("CARGO_PKG_VERSION")));

    let unknown = Run::new(&["--not-a-flag"]).go();
    assert_eq!(unknown.code, 2);
    assert_eq!(unknown.stdout, "");
    assert!(unknown.stderr.starts_with("jev: "), "{}", unknown.stderr);
    assert_eq!(unknown.stderr.lines().count(), 1);

    let bare = Run::new(&[]).go();
    assert_eq!(bare.code, 2);
    assert!(bare.stderr.contains("subcommand"), "{}", bare.stderr);
}

#[test]
fn state_and_questions_can_be_files() {
    let mut state = tempfile::NamedTempFile::new().unwrap();
    writeln!(state, "hello from a file").unwrap();
    let path = state.path().to_str().unwrap();
    let out = Run::new(&["noul", "Is it empty?", "-f", path, "--api-key", "test", "--retries", "0"])
        .transport(ok_body(r#"{"model":"m","answers":{"q":{"type":"noul","noul":0.25}},"usage":{}}"#))
        .go();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "0.25\n");

    let mut questions = tempfile::NamedTempFile::new().unwrap();
    write!(questions, "{}", include_questions()).unwrap();
    let qpath = questions.path().to_str().unwrap();
    let asked = Run::new(&["ask", "-q", qpath, "-s", "Please help ASAP", "--dry-run"]).go();
    assert_eq!(asked.code, 0, "{}", asked.stderr);
    assert!(asked.stdout.contains("frustration"), "{}", asked.stdout);
}

impl Run {
    fn arg_tail(mut self, extra: &[&str]) -> Self {
        self.args.extend(extra.iter().map(OsString::from));
        self
    }
}

fn include_questions() -> String {
    let example = Run::new(&["example"]).go();
    assert_eq!(example.code, 0, "{}", example.stderr);
    example.stdout
}

struct ExplodingStdin;

impl Read for ExplodingStdin {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("stdin was read"))
    }
}

enum Broken {
    Write,
    Flush,
}

impl Write for Broken {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        match self {
            Self::Write => Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")),
            Self::Flush => Ok(data.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Write => Ok(()),
            Self::Flush => Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed")),
        }
    }
}

const CI_PRESET: &str = include_str!("../tests/fixtures/presets/ci.json");

/// A flaky CI failure for the CI preset: rule 3 chooses `retry`.
const FLAKY: &str = r#"{
    "model": "jev-1.13.0",
    "answers": {
        "secrets": {"type": "noul", "noul": 0.03},
        "kind": {
            "type": "choice",
            "choice": "retry",
            "confidence": 0.84,
            "probabilities": {"retry": 0.90, "fmt": 0.02, "issue": 0.08}
        },
        "scope": {
            "type": "score",
            "score": 0.3,
            "confidence": 0.7,
            "legend": {"0": "One test or one file", "1": "Several tests or modules", "2": "Most of the build"},
            "probabilities": {"0": 0.75, "1": 0.2, "2": 0.05}
        }
    },
    "usage": {"input_tokens": 900, "output_tokens": 60}
}"#;

const FLAKY_EXPLAIN: &str = "\
decision: retry (rule 3)
rule 1  no   secrets 0.03 at_least 0.30          margin -0.27
rule 2  no   kind retry is issue                 margin -0.82
             scope 0.30 at_least 1.50            margin -0.60
rule 3  yes  kind confidence 0.84 at_least 0.60  margin +0.24
closest call: rule 3, kind confidence (+0.24)

secrets: 0.03
kind: retry (confidence 0.84)
  retry: 0.90
  fmt: 0.02
  issue: 0.08
scope: 0.30 (confidence 0.70)
  0 One test or one file: 0.75
  1 Several tests or modules: 0.20
  2 Most of the build: 0.05
";

/// A directory holding `preset.json`, which is `preset` with every action's `run` replaced by
/// `run`, so the tests control what runs.
struct PresetDir {
    dir: tempfile::TempDir,
}

impl PresetDir {
    fn new(preset: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("preset.json"), preset).unwrap();
        Self { dir }
    }

    /// `CI_PRESET` with every action running `run`.
    fn ci_running(run: &[&str]) -> Self {
        let mut preset: Value = serde_json::from_str(CI_PRESET).unwrap();
        for action in preset["actions"].as_object_mut().unwrap().values_mut() {
            action["run"] = serde_json::json!(run);
        }
        Self::new(&preset.to_string())
    }

    fn preset(&self) -> String {
        self.dir.path().join("preset.json").display().to_string()
    }

    fn script(&self, name: &str, body: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        path.display().to_string()
    }
}

/// The request arguments for a preset command, with a fake key and no retries.
fn preset_args<'a>(command: &'a str, preset: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args =
        vec![command, preset, "-s", "error: connection reset by peer", "--api-key", "test", "--retries", "0"];
    args.extend_from_slice(extra);
    args
}

#[test]
fn decide_prints_the_action_and_json_puts_the_decision_first() {
    let dir = PresetDir::new(CI_PRESET);
    let preset = dir.preset();
    let text = Run::new(&preset_args("decide", &preset, &[])).transport(ok_body(FLAKY)).go();
    assert_eq!((text.code, text.stdout.as_str(), text.stderr.as_str()), (0, "retry\n", ""));

    let json = Run::new(&preset_args("decide", &preset, &["--json"])).transport(ok_body(FLAKY)).go();
    assert_eq!(json.code, 0, "{}", json.stderr);
    let keys: Vec<String> =
        serde_json::from_str::<serde_json::Map<String, Value>>(&json.stdout).unwrap().keys().cloned().collect();
    assert_eq!(keys, ["decision", "rule", "rules", "model", "answers", "usage", "cost_usd"]);
    let value: Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!((&value["decision"], &value["rule"]), (&serde_json::json!("retry"), &serde_json::json!(3)));
    let matched: Vec<&Value> = value["rules"].as_array().unwrap().iter().map(|rule| &rule["matched"]).collect();
    assert_eq!(matched, [false, false, true]);
    let is = &value["rules"][1]["conditions"][0];
    let keys: Vec<&String> = is.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["question", "test", "option", "value", "met", "margin"]);
    assert_eq!(
        (&is["test"], &is["option"], &is["value"], &is["met"]),
        (&"is".into(), &"issue".into(), &"retry".into(), &false.into())
    );
    assert!((is["margin"].as_f64().unwrap() + 0.82).abs() < 1e-9, "{is}");
    let threshold = &value["rules"][0]["conditions"][0];
    assert_eq!(
        (&threshold["test"], &threshold["threshold"], &threshold["value"]),
        (&"at_least".into(), &0.3.into(), &0.03.into())
    );
    assert!(value["cost_usd"].is_number());
}

#[test]
fn explain_writes_every_rule_and_the_answers_to_stderr() {
    let dir = PresetDir::new(CI_PRESET);
    let preset = dir.preset();
    let out = Run::new(&preset_args("decide", &preset, &["--explain", "-v"])).transport(ok_body(FLAKY)).go();
    assert_eq!(out.code, 0);
    assert_eq!(out.stdout, "retry\n");
    let verbose = "jev-1.13.0: 900 input tokens, 60 output tokens, 0 retries, ~$0.000038\n";
    assert_eq!(out.stderr, format!("{FLAKY_EXPLAIN}{verbose}"));

    let unsure = FLAKY.replace("\"confidence\": 0.84", "\"confidence\": 0.41");
    let out = Run::new(&preset_args("decide", &preset, &["--explain"])).transport(status_body(200, unsure)).go();
    assert_eq!(out.stdout, "escalate\n");
    assert!(out.stderr.starts_with("decision: escalate (fallback)\n"), "{}", out.stderr);
    assert!(out.stderr.contains("rule 3  no   kind confidence 0.41 at_least 0.60  margin -0.19\n"), "{}", out.stderr);
    assert!(out.stderr.contains("closest call: rule 3, kind confidence (-0.19)\n"), "{}", out.stderr);
}

#[test]
fn no_match_without_a_fallback_exits_1() {
    let mut preset: Value = serde_json::from_str(CI_PRESET).unwrap();
    preset.as_object_mut().unwrap().remove("fallback");
    let dir = PresetDir::new(&preset.to_string());
    let path = dir.preset();
    let unsure = FLAKY.replace("\"confidence\": 0.84", "\"confidence\": 0.41");
    let text = Run::new(&preset_args("decide", &path, &[])).transport(status_body(200, unsure.clone())).go();
    assert_eq!((text.code, text.stdout.as_str(), text.stderr.as_str()), (1, "", ""));
    let json = Run::new(&preset_args("decide", &path, &["--json"])).transport(status_body(200, unsure)).go();
    assert_eq!(json.code, 1);
    let value: Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!((&value["decision"], &value["rule"]), (&Value::Null, &Value::Null));
}

#[test]
fn a_bad_preset_fails_before_the_state_is_read_or_a_request_is_sent() {
    let bad = CI_PRESET.replace(r#"{ "is": "issue" }"#, r#"{ "is": "issues" }"#);
    let dir = PresetDir::new(&bad);
    let preset = dir.preset();
    let mut io = Io {
        args: ["jev", "decide", preset.as_str(), "--api-key", "test"].into_iter().map(OsString::from).collect(),
        stdin: ExplodingStdin,
        stdout: Vec::new(),
        stderr: Arc::new(Mutex::new(Vec::new())),
        stdin_is_terminal: false,
        env: Some(BTreeMap::new()),
    };
    assert_eq!(run(&mut io, Some(Box::new(refuse()))), 2);
    let stderr = String::from_utf8(io.stderr.lock().unwrap().clone()).unwrap();
    assert_eq!(stderr, format!("jev: preset {preset}: rule 2: kind: `issues` is not an option of `kind`\n"));

    let mut no_fallback: Value = serde_json::from_str(CI_PRESET).unwrap();
    no_fallback.as_object_mut().unwrap().remove("fallback");
    let dir = PresetDir::new(&no_fallback.to_string());
    let preset = dir.preset();
    let decide = Run::new(&["decide", &preset, "-s", "x", "--dry-run"]).transport(refuse()).go();
    assert_eq!(decide.code, 0, "decide does not need a fallback: {}", decide.stderr);
    let call = Run::new(&["call", &preset, "-s", "x", "--dry-run"]).transport(refuse()).go();
    assert_eq!((call.code, call.stdout.as_str()), (2, ""));
    assert_eq!(call.stderr, format!("jev: preset {preset}: call needs a fallback action\n"));

    let missing = Run::new(&["decide", "/nonexistent/preset.json", "-s", "x"]).transport(refuse()).go();
    assert_eq!(missing.code, 7);
    assert!(
        missing.stderr.starts_with("jev: cannot read preset from /nonexistent/preset.json: "),
        "{}",
        missing.stderr
    );

    let unnamed = Run::new(&["decide", "no-such-preset-name", "-s", "x"]).transport(refuse()).go();
    assert_eq!(unnamed.code, 2);
    assert!(
        unnamed.stderr.starts_with("jev: no preset named `no-such-preset-name` in .jev/presets"),
        "{}",
        unnamed.stderr
    );
}

#[test]
fn a_dry_run_prints_the_preset_questions_and_needs_no_key() {
    let dir = PresetDir::new(CI_PRESET);
    let preset = dir.preset();
    for command in ["decide", "call"] {
        let out = Run::new(&[command, &preset, "-s", "a log", "--dry-run"]).transport(refuse()).go();
        assert_eq!(out.code, 0, "{command}: {}", out.stderr);
        let body: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(body["state"], "a log");
        let ids: Vec<&String> = body["questions"].as_object().unwrap().keys().collect();
        assert_eq!(ids, ["secrets", "kind", "scope"]);
        assert_eq!(out.stderr, "");
    }
}

#[test]
fn the_preset_can_come_from_stdin_but_not_together_with_the_state() {
    let out = Run::new(&["decide", "-", "-s", "log", "--api-key", "test", "--retries", "0"])
        .stdin(CI_PRESET)
        .transport(ok_body(FLAKY))
        .go();
    assert_eq!((out.code, out.stdout.as_str()), (0, "retry\n"), "{}", out.stderr);
    let both = Run::new(&["decide", "-"]).stdin(CI_PRESET).transport(refuse()).go();
    assert_eq!((both.code, both.stderr.as_str()), (2, "jev: state and preset cannot both be read from stdin\n"));
    let example = Run::new(&["example", "--preset"]).go();
    let piped = Run::new(&["call", "-", "-s", EXAMPLE_STATE, "--dry-run"]).stdin(&example.stdout).go();
    assert_eq!(piped.code, 0, "{}", piped.stderr);
    assert!(piped.stdout.contains("\"department\""), "{}", piped.stdout);
}

#[cfg(unix)]
#[test]
fn call_runs_the_action_with_the_state_on_stdin_and_returns_its_status() {
    let script = "\
printf 'action=%s\\n' \"$JEV_ACTION\"
printf 'key=%s marker=%s\\n' \"${TYPESAFE_API_KEY-unset}\" \"$MARKER\"
printf 'args=%s|%s\\n' \"$1\" \"$2\"
printf 'stdin=' ; cat
printf 'to stderr\\n' >&2
exit 3
";
    let placeholder = PresetDir::ci_running(&["/bin/sh"]);
    let path = placeholder.script("act.sh", script);
    let dir = PresetDir::ci_running(&["/bin/sh", &path, "a b", "$HOME"]);
    let preset = dir.preset();
    let out = Run::new(&preset_args("call", &preset, &[]))
        .env("PATH", "/usr/bin:/bin")
        .env("MARKER", "from-env")
        .transport(ok_body(FLAKY))
        .go();
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "action=retry\nkey=unset marker=from-env\nargs=a b|$HOME\nstdin=error: connection reset by peer"
    );
    assert_eq!(out.stderr, "to stderr\n");

    let json = Run::new(&preset_args("call", &preset, &["--json"]))
        .env("PATH", "/usr/bin:/bin")
        .transport(ok_body(FLAKY))
        .go();
    assert_eq!(json.code, 3, "{}", json.stderr);
    assert_eq!(json.stderr, "");
    let value: Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(value["decision"], "retry");
    assert_eq!(value["result"]["status"], 3);
    assert!(value["result"]["stdout"].as_str().unwrap().starts_with("action=retry\n"), "{value}");
    assert_eq!(value["result"]["stderr"], "to stderr\n");
    let keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys.last().map(String::as_str), Some("result"));
}

#[cfg(unix)]
#[test]
fn the_action_gets_the_decision_and_the_preset_in_its_environment() {
    let placeholder = PresetDir::ci_running(&["/bin/sh"]);
    let path = placeholder.script("env.sh", "printf '%s\\n%s' \"$JEV_PRESET\" \"$JEV_DECISION\"\n");
    let dir = PresetDir::ci_running(&["/bin/sh", &path]);
    let preset = dir.preset();
    let out = Run::new(&preset_args("call", &preset, &["--explain"])).transport(ok_body(FLAKY)).go();
    assert_eq!(out.code, 0, "{}", out.stderr);
    let (label, decision) = out.stdout.split_once('\n').unwrap();
    assert_eq!(label, preset);
    let decision: Value = serde_json::from_str(decision).unwrap();
    let decided = Run::new(&preset_args("decide", &preset, &["--json"])).transport(ok_body(FLAKY)).go();
    let expected: Value = serde_json::from_str(&decided.stdout).unwrap();
    assert_eq!(decision, expected, "JEV_DECISION is the `decide --json` document");
    assert_eq!(out.stderr, FLAKY_EXPLAIN, "--explain is written before the action runs");
}

#[cfg(unix)]
#[test]
fn a_relative_program_is_resolved_against_the_preset_directory() {
    // A symlink to the system shell, not a script: a file this test had just written could still
    // be open in another test thread's fork, and exec would fail with ETXTBSY.
    let dir = PresetDir::ci_running(&["./bin/sh", "-c", "echo ran \"$JEV_ACTION\""]);
    std::fs::create_dir(dir.dir.path().join("bin")).unwrap();
    std::os::unix::fs::symlink("/bin/sh", dir.dir.path().join("bin").join("sh")).unwrap();
    let preset = dir.preset();
    let out = Run::new(&preset_args("call", &preset, &[])).env("PATH", "/usr/bin:/bin").transport(ok_body(FLAKY)).go();
    assert_eq!((out.code, out.stdout.as_str(), out.stderr.as_str()), (0, "ran retry\n", ""));
}

#[cfg(unix)]
#[test]
fn a_signal_is_128_plus_n_and_a_missing_program_is_exit_7() {
    let placeholder = PresetDir::ci_running(&["/bin/sh"]);
    let path = placeholder.script("die.sh", "kill -TERM $$\n");
    let dir = PresetDir::ci_running(&["/bin/sh", &path]);
    let preset = dir.preset();
    let out = Run::new(&preset_args("call", &preset, &[])).transport(ok_body(FLAKY)).go();
    assert_eq!(out.code, 128 + 15, "{}", out.stderr);

    let dir = PresetDir::ci_running(&["./missing.sh"]);
    let preset = dir.preset();
    let out = Run::new(&preset_args("call", &preset, &[])).transport(ok_body(FLAKY)).go();
    assert_eq!(out.code, 7);
    let program = dir.dir.path().join("./missing.sh");
    assert!(
        out.stderr.starts_with(&format!("jev: cannot run action `retry` ({}): ", program.display())),
        "{}",
        out.stderr
    );
}

#[cfg(unix)]
#[test]
fn the_example_preset_runs_end_to_end() {
    let example = Run::new(&["example", "--preset"]).go();
    let dir = PresetDir::new(&example.stdout);
    let preset = dir.preset();
    let out = Run::new(&["call", &preset, "-s", EXAMPLE_STATE, "--api-key", "test", "--retries", "0"])
        .env("PATH", "/usr/bin:/bin")
        .transport(ok_body(ASK))
        .go();
    assert_eq!((out.code, out.stdout.as_str(), out.stderr.as_str()), (0, "technical queue\n", ""));
}

#[test]
fn the_readme_embeds_the_example_preset_and_its_explanation() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let after = readme.split_once("`jev example --preset` prints this preset").unwrap().1;
    let block = after.split_once("```json\n").unwrap().1.split_once("\n```").unwrap().0;
    let documented: Value = serde_json::from_str(block).unwrap();
    let example = Run::new(&["example", "--preset"]).go();
    let printed: Value = serde_json::from_str(&example.stdout).unwrap();
    assert_eq!(documented, printed, "the README's preset is not `jev example --preset`");

    let dir = PresetDir::new(&example.stdout);
    let preset = dir.preset();
    let explained = Run::new(&["decide", &preset, "-s", EXAMPLE_STATE, "--explain", "--api-key", "test"])
        .transport(ok_body(ASK))
        .go();
    assert_eq!(explained.stdout, "technical\n");
    assert!(readme.contains(&explained.stderr), "README is missing this explanation:\n{}", explained.stderr);
}
