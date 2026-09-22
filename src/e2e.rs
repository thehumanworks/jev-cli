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
