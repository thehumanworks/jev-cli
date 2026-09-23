//! Orchestrate one invocation: parse, read, send, render.
//!
//! An injected transport never sleeps between retries (`backoff_scale` is zero) so the suite stays
//! fast. The real client keeps the crate's backoff. `--timeout` is applied to both the request
//! timeout and the connect timeout, so one flag bounds the whole call.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{ErrorKind as IoErrorKind, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::error::ErrorKind;
use clap_complete::generate;
use serde_json::Value;
use typesafe_jev::{Client, Config, Response, Transport};

use crate::call;
use crate::cli::{
    AskArgs, ChoiceArgs, Command, CommonArgs, ExampleArgs, Explicit, NoulArgs, OutputFormat, Parsed, PresetArgs,
    ScoreArgs, apply_env, command, common_mut, output_format, parse,
};
use crate::decide::decide;
use crate::exit::{Failure, one_line};
use crate::preset::{Source, example_preset, locate, parse_preset};
use crate::questions::{
    MISSING_QUESTIONS, check_threshold, example_questions, into_choice, noul_question, one, parse_options,
    questions_from_json, score_question,
};
use crate::render::{
    decision_document, pretty, render_ask_text, render_choice_text, render_explain, render_noul_text, render_request,
    render_response, render_score_text, required_noul, verbose_line,
};
use crate::state::{EXAMPLE_STATE, Origin, State, TERMINAL_STATE, interpret_state, read_origin, strip_bom};

/// State and questions were both aimed at stdin.
const BOTH_STDIN: &str = "state and questions cannot both be read from stdin";

/// State and preset were both aimed at stdin.
const BOTH_STDIN_PRESET: &str = "state and preset cannot both be read from stdin";

/// No key was passed and none was in the environment.
const NO_API_KEY: &str = "no API key; pass --api-key or set TYPESAFE_API_KEY";

/// Process inputs and outputs for one run.
///
/// `main` fills this from the real process. Tests pass in-memory buffers and set
/// [`Io::stdin_is_terminal`] themselves. [`Io::env`] `None` reads the process environment;
/// `Some` is the whole environment this run will consult, including when the map is empty.
pub struct Io<I, O, E> {
    /// Arguments, including the program name at index 0.
    pub args: Vec<OsString>,
    /// Standard input.
    pub stdin: I,
    /// Standard output. A broken pipe here exits 0 and writes no error.
    pub stdout: O,
    /// Standard error. Shared so `--debug` can write each failed attempt as it happens.
    pub stderr: Arc<Mutex<E>>,
    /// When true, and neither `--state` nor `--state-file` was given, reading stdin is a usage error.
    pub stdin_is_terminal: bool,
    /// Environment for `TYPESAFE_API_KEY`, `JEV_BASE_URL`, `JEV_MODEL`, `JEV_TIMEOUT`, `JEV_RETRIES`, and
    /// `XDG_CONFIG_HOME` and `HOME` for finding a preset by name. `Some` is also the whole environment
    /// of an action that `call` runs, plus `JEV_PRESET`, `JEV_ACTION` and `JEV_DECISION`.
    pub env: Option<BTreeMap<String, String>>,
}

/// Run one invocation.
///
/// `transport` `None` talks to `base_url` over HTTP. `Some` handles the POST itself and does not
/// open a socket; retries then wait zero time. The return value is the process status.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use std::io::Cursor;
/// use typesafe_jev::{Reply, Transport};
///
/// let mut io = jev::Io {
///     args: ["jev", "noul", "Is it empty?", "-s", "hello", "--api-key", "test", "--retries", "0"]
///         .into_iter()
///         .map(std::ffi::OsString::from)
///         .collect(),
///     stdin: Cursor::new(Vec::<u8>::new()),
///     stdout: Vec::new(),
///     stderr: std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new())),
///     stdin_is_terminal: false,
///     env: Some(BTreeMap::new()),
/// };
/// let transport: Box<dyn Transport> = Box::new(|_body: &[u8]| {
///     Ok(Reply {
///         status: 200,
///         retry_after: None,
///         body: r#"{"model":"fake","answers":{"q":{"type":"noul","noul":0.5}},"usage":{}}"#.into(),
///     })
/// });
/// let code = jev::run(&mut io, Some(transport));
/// assert_eq!(code, 0);
/// assert_eq!(String::from_utf8(io.stdout).unwrap(), "0.5\n");
/// ```
#[must_use]
pub fn run<I, O, E>(io: &mut Io<I, O, E>, transport: Option<Box<dyn Transport>>) -> u8
where
    I: Read,
    O: Write,
    E: Write + Send + 'static,
{
    match parse(&io.args) {
        Err(error) => clap_exit(io, &error),
        Ok(parsed) => execute(io, parsed, transport),
    }
}

fn execute<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    parsed: Parsed,
    transport: Option<Box<dyn Transport>>,
) -> u8 {
    match execute_inner(io, parsed, transport) {
        Ok(code) => code,
        Err(Failure::BrokenPipe) => 0,
        Err(failure) => {
            report(&io.stderr, &failure);
            failure.code()
        }
    }
}

fn execute_inner<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    mut parsed: Parsed,
    transport: Option<Box<dyn Transport>>,
) -> Result<u8, Failure> {
    let explicit = parsed.explicit;
    if let Some(common) = common_mut(&mut parsed.command) {
        apply_env(common, explicit, io.env.as_ref())?;
    }
    match parsed.command {
        Command::Example(args) => print_example(&mut io.stdout, &args),
        Command::Completions(args) => print_completions(&mut io.stdout, args.shell),
        Command::Ask(args) => run_ask(io, &args, explicit, transport),
        Command::Noul(args) => run_noul(io, &args, explicit, transport),
        Command::Choice(args) => run_choice(io, &args, explicit, transport),
        Command::Score(args) => run_score(io, &args, explicit, transport),
        Command::Decide(args) => run_preset(io, &args, explicit, transport, false),
        Command::Call(args) => run_preset(io, &args, explicit, transport, true),
    }
}

fn run_ask<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    args: &AskArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
) -> Result<u8, Failure> {
    // Arguments are checked before any read, so a bad document or flag wins over a missing state.
    output_format(&args.common, explicit)?;
    let origin = questions_origin(args)?;
    if state_uses_stdin(&args.common) && matches!(origin, QOrigin::Stdin) {
        return Err(Failure::Usage(BOTH_STDIN.into()));
    }
    let questions = load_questions(&origin, &mut io.stdin)?;
    let state = load_state(&args.common, &mut io.stdin, io.stdin_is_terminal)?;
    present(io, &args.common, explicit, transport, &state, &questions, &Mode::Ask)
}

fn run_noul<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    args: &NoulArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
) -> Result<u8, Failure> {
    output_format(&args.common, explicit)?;
    if let Some(probability) = args.threshold {
        check_threshold(probability)?;
    }
    let question = noul_question(&args.instructions, args.yes.as_deref(), args.no.as_deref());
    let questions = one(&args.id, question)?;
    let state = load_state(&args.common, &mut io.stdin, io.stdin_is_terminal)?;
    present(
        io,
        &args.common,
        explicit,
        transport,
        &state,
        &questions,
        &Mode::Noul { id: &args.id, threshold: args.threshold },
    )
}

fn run_choice<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    args: &ChoiceArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
) -> Result<u8, Failure> {
    output_format(&args.common, explicit)?;
    let options = parse_options(&args.options)?;
    let question = into_choice(&args.instructions, &options);
    let questions = one(&args.id, question)?;
    let state = load_state(&args.common, &mut io.stdin, io.stdin_is_terminal)?;
    present(io, &args.common, explicit, transport, &state, &questions, &Mode::Choice { id: &args.id })
}

fn run_score<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    args: &ScoreArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
) -> Result<u8, Failure> {
    output_format(&args.common, explicit)?;
    let question = score_question(&args.instructions, &args.levels)?;
    let questions = one(&args.id, question)?;
    let state = load_state(&args.common, &mut io.stdin, io.stdin_is_terminal)?;
    present(io, &args.common, explicit, transport, &state, &questions, &Mode::Score { id: &args.id })
}

/// `decide`, or with `call` set, `call`. The preset is read and checked, `call`'s extra checks
/// included, before the state is read and before any request.
fn run_preset<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    args: &PresetArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
    call: bool,
) -> Result<u8, Failure> {
    let format = output_format(&args.common, explicit)?;
    let source = locate(&args.preset, std::env::current_dir, io.env.as_ref())?;
    if state_uses_stdin(&args.common) && matches!(source, Source::Stdin) {
        return Err(Failure::Usage(BOTH_STDIN_PRESET.into()));
    }
    let origin = match &source {
        Source::Stdin => Origin::Stdin,
        Source::File(path) => Origin::File(path),
    };
    let text = read_origin(&origin, "preset", &mut io.stdin)?;
    let label = source.label();
    let named = |failure: Failure| Failure::Usage(format!("preset {label}: {}", failure.message()));
    let preset = parse_preset(&text).map_err(named)?;
    if call {
        preset.check_callable().map_err(named)?;
    }
    let raw = read_state(&args.common, &mut io.stdin, io.stdin_is_terminal)?;
    let state = interpret_state(&raw, args.common.state_json)?;
    let conn = connection_from(&args.common);
    if args.common.dry_run {
        let body = render_request(&conn.model, &state, &preset.questions)?;
        write_stdout(&mut io.stdout, &body)?;
        return Ok(0);
    }
    let asked = send(&io.stderr, &conn, &args.common, &state, &preset.questions, transport)?;
    let decision = decide(&preset, &asked.response)?;
    let mut document = decision_document(&decision, &asked.response, asked.cost_usd)?;
    if args.explain {
        write_quiet(&io.stderr, &render_explain(&preset, &decision, &asked.response)?);
    }
    if !call {
        let text = match (format, &decision.action) {
            (OutputFormat::Json, _) => pretty(&Value::Object(document))?,
            (OutputFormat::Text, Some(action)) => format!("{action}\n"),
            (OutputFormat::Text, None) => String::new(),
        };
        write_stdout(&mut io.stdout, &text)?;
        if let Some(line) = &asked.verbose {
            write_quiet(&io.stderr, line);
        }
        return Ok(u8::from(decision.action.is_none()));
    }
    if let Some(line) = &asked.verbose {
        write_quiet(&io.stderr, line);
    }
    // `check_callable` made these hold: there is a fallback, and every action has a program.
    let name = decision.action.as_deref().ok_or_else(|| Failure::Usage("call needs a fallback action".into()))?;
    let Some((program, rest)) =
        preset.action(name).and_then(|action| action.run.as_deref()).and_then(<[_]>::split_first)
    else {
        return Err(Failure::Usage(format!("call needs a run for every action, and `{name}` has none")));
    };
    let compact =
        serde_json::to_string(&document).map_err(|err| Failure::Api(format!("cannot format the decision: {err}")))?;
    let action = call::Action {
        name,
        program: resolve_program(program, source.directory()),
        args: rest,
        stdin: strip_bom(&raw).as_bytes().to_vec(),
        env: io.env.as_ref(),
        extra_env: vec![("JEV_PRESET", label.clone()), ("JEV_ACTION", name.to_owned()), ("JEV_DECISION", compact)],
    };
    if format == OutputFormat::Text {
        return call::stream(action, &mut io.stdout, &io.stderr);
    }
    let collected = call::collect(action)?;
    let mut result = serde_json::Map::new();
    result.insert("status".to_owned(), Value::from(collected.status));
    result.insert("stdout".to_owned(), Value::String(String::from_utf8_lossy(&collected.stdout).into_owned()));
    result.insert("stderr".to_owned(), Value::String(String::from_utf8_lossy(&collected.stderr).into_owned()));
    document.insert("result".to_owned(), Value::Object(result));
    // The action has run, so its status stands even when stdout is closed.
    match write_stdout(&mut io.stdout, &pretty(&Value::Object(document))?) {
        Ok(()) | Err(Failure::BrokenPipe) => Ok(collected.status),
        Err(failure) => Err(failure),
    }
}

/// A relative program path with a `/` in it is relative to the preset's directory. A bare name is
/// looked up on `PATH`.
fn resolve_program(program: &str, directory: Option<&std::path::Path>) -> std::path::PathBuf {
    let path = std::path::Path::new(program);
    match directory {
        Some(directory) if path.is_relative() && program.contains('/') => directory.join(path),
        _ => path.to_path_buf(),
    }
}

enum Mode<'a> {
    Ask,
    Noul { id: &'a str, threshold: Option<f64> },
    Choice { id: &'a str },
    Score { id: &'a str },
}

fn present<I: Read, O: Write, E: Write + Send + 'static>(
    io: &mut Io<I, O, E>,
    common: &CommonArgs,
    explicit: Explicit,
    transport: Option<Box<dyn Transport>>,
    state: &State,
    questions: &typesafe_jev::Questions,
    mode: &Mode<'_>,
) -> Result<u8, Failure> {
    let format = output_format(common, explicit)?;
    let conn = connection_from(common);
    if common.dry_run {
        let body = render_request(&conn.model, state, questions)?;
        write_stdout(&mut io.stdout, &body)?;
        return Ok(0);
    }
    let asked = send(&io.stderr, &conn, common, state, questions, transport)?;
    let rendered = render_mode(format, mode, questions, &asked)?;
    let code = match mode {
        Mode::Noul { id, threshold } => threshold_code(*threshold, &asked.response, id)?,
        Mode::Ask | Mode::Choice { .. } | Mode::Score { .. } => 0,
    };
    write_stdout(&mut io.stdout, &rendered)?;
    if let Some(line) = &asked.verbose {
        write_quiet(&io.stderr, line);
    }
    Ok(code)
}

fn render_mode(
    format: OutputFormat,
    mode: &Mode<'_>,
    questions: &typesafe_jev::Questions,
    asked: &Asked,
) -> Result<String, Failure> {
    if format == OutputFormat::Json {
        return render_response(&asked.response, asked.cost_usd);
    }
    match mode {
        Mode::Ask => render_ask_text(questions, &asked.response),
        Mode::Noul { id, .. } => render_noul_text(&asked.response, id),
        Mode::Choice { id } => render_choice_text(&asked.response, id),
        Mode::Score { id } => render_score_text(&asked.response, id),
    }
}

fn threshold_code(threshold: Option<f64>, response: &Response, id: &str) -> Result<u8, Failure> {
    let Some(threshold) = threshold else {
        return Ok(0);
    };
    let answer = required_noul(response, id)?;
    // A non-finite probability does not meet the threshold.
    Ok(u8::from(!(answer.noul.is_finite() && answer.noul >= threshold)))
}

struct Connection {
    api_key: Option<String>,
    base_url: String,
    model: String,
    timeout: Duration,
    retries: u32,
}

fn connection_from(common: &CommonArgs) -> Connection {
    Connection {
        api_key: common.api_key.clone(),
        base_url: common.base_url.clone(),
        model: common.model.clone(),
        timeout: Duration::from_secs(common.timeout),
        retries: common.retries,
    }
}

struct Asked {
    response: Response,
    verbose: Option<String>,
    cost_usd: f64,
}

fn send<E: Write + Send + 'static>(
    stderr: &Arc<Mutex<E>>,
    conn: &Connection,
    common: &CommonArgs,
    state: &State,
    questions: &typesafe_jev::Questions,
    transport: Option<Box<dyn Transport>>,
) -> Result<Asked, Failure> {
    let key = conn.api_key.as_deref().ok_or_else(|| Failure::Auth(NO_API_KEY.into()))?;
    let mut cfg = Config::default();
    cfg.base_url.clone_from(&conn.base_url);
    cfg.model.clone_from(&conn.model);
    cfg.timeout = conn.timeout;
    cfg.connect_timeout = conn.timeout;
    cfg.max_retries = conn.retries;
    let mut client = match transport {
        Some(transport) => {
            // Injected transports are how tests avoid the network. Do not sleep between retries.
            cfg.backoff_scale = 0.0;
            Client::with_transport(transport, cfg)
        }
        None => Client::new(key, cfg).map_err(|err| Failure::from_api(&err, common.debug))?,
    };
    if common.debug {
        let stderr = Arc::clone(stderr);
        client.set_debug_reporter(move |line| {
            let mut guard = stderr.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(writeln!(&mut *guard, "{line}"));
            drop(guard.flush());
        });
    }
    let response = client.ask(state, questions).map_err(|err| Failure::from_api(&err, common.debug))?;
    let usage = client.usage();
    let cost_usd = usage.cost_usd();
    let verbose = common
        .verbose
        .then(|| verbose_line(&response.model, usage.input_tokens(), usage.output_tokens(), usage.retries(), cost_usd));
    Ok(Asked { response, verbose, cost_usd })
}

#[derive(Clone, Copy)]
enum QOrigin<'a> {
    Inline(&'a str),
    File(&'a std::path::Path),
    Stdin,
}

fn questions_origin(args: &AskArgs) -> Result<QOrigin<'_>, Failure> {
    match (&args.questions, &args.questions_json) {
        (None, None) => Err(Failure::Usage(MISSING_QUESTIONS.into())),
        (Some(_), Some(_)) => {
            // clap's conflicts_with already rejects this; the arm keeps the match total
            Err(Failure::Usage("pass only one of --questions and --questions-json".into()))
        }
        (None, Some(json)) => Ok(QOrigin::Inline(json)),
        (Some(path), None) if path.as_os_str() == "-" => Ok(QOrigin::Stdin),
        (Some(path), None) => Ok(QOrigin::File(path)),
    }
}

fn load_questions(origin: &QOrigin<'_>, stdin: &mut dyn Read) -> Result<typesafe_jev::Questions, Failure> {
    let raw = match *origin {
        QOrigin::Inline(json) => json.to_owned(),
        QOrigin::File(path) => read_origin(&Origin::File(path), "questions", stdin)?,
        QOrigin::Stdin => read_origin(&Origin::Stdin, "questions", stdin)?,
    };
    questions_from_json(&raw)
}

fn load_state(common: &CommonArgs, stdin: &mut dyn Read, reject_terminal: bool) -> Result<State, Failure> {
    let raw = read_state(common, stdin, reject_terminal)?;
    interpret_state(&raw, common.state_json)
}

/// The state text as read, before it is checked.
fn read_state(common: &CommonArgs, stdin: &mut dyn Read, reject_terminal: bool) -> Result<String, Failure> {
    if reject_terminal && implicit_stdin(common) {
        return Err(Failure::Usage(TERMINAL_STATE.into()));
    }
    let origin = match (&common.state, &common.state_file) {
        (Some(text), None) => Origin::Text(text),
        (None, Some(path)) if path.as_os_str() == "-" => Origin::Stdin,
        (None, Some(path)) => Origin::File(path),
        (None, None) => Origin::Stdin,
        (Some(_), Some(_)) => {
            // clap's conflicts_with already rejects this; the arm keeps the match total
            return Err(Failure::Usage("pass only one of --state and --state-file".into()));
        }
    };
    read_origin(&origin, "state", stdin)
}

fn implicit_stdin(common: &CommonArgs) -> bool {
    common.state.is_none() && common.state_file.is_none()
}

fn state_uses_stdin(common: &CommonArgs) -> bool {
    match &common.state_file {
        Some(path) => common.state.is_none() && path.as_os_str() == "-",
        None => common.state.is_none(),
    }
}

fn print_example(stdout: &mut impl Write, args: &ExampleArgs) -> Result<u8, Failure> {
    let text = if args.state {
        format!("{EXAMPLE_STATE}\n")
    } else if args.preset {
        pretty(&example_preset()?)?
    } else {
        pretty(&example_questions())?
    };
    write_stdout(stdout, &text)?;
    Ok(0)
}

fn print_completions(stdout: &mut impl Write, shell: clap_complete::Shell) -> Result<u8, Failure> {
    let mut cmd = command();
    let mut buf = Vec::new();
    generate(shell, &mut cmd, "jev", &mut buf);
    let text = String::from_utf8(buf).map_err(|_| Failure::Api("the completion script is not UTF-8".into()))?;
    write_stdout(stdout, &text)?;
    Ok(0)
}

fn clap_exit<I, O: Write, E: Write>(io: &mut Io<I, O, E>, error: &clap::Error) -> u8 {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => match write_stdout(&mut io.stdout, &error.to_string()) {
            Ok(()) | Err(Failure::BrokenPipe) => 0,
            Err(failure) => {
                report(&io.stderr, &failure);
                failure.code()
            }
        },
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            report_message(&io.stderr, "a subcommand is required; try `jev --help`");
            2
        }
        _ => {
            report_message(&io.stderr, &clap_message(error));
            2
        }
    }
}

/// The error text up to the blank line before clap's `Usage:` block, as one line.
fn clap_message(error: &clap::Error) -> String {
    let rendered = error.to_string();
    let mut parts = Vec::new();
    for line in rendered.lines() {
        if line.is_empty() {
            if !parts.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with("Usage:") {
            break;
        }
        parts.push(line.strip_prefix("error: ").unwrap_or(line));
    }
    one_line(&parts.join(" "))
}

fn write_stdout(stdout: &mut impl Write, text: &str) -> Result<(), Failure> {
    match stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == IoErrorKind::BrokenPipe => Err(Failure::BrokenPipe),
        Err(err) => Err(Failure::Io(format!("cannot write stdout: {err}"))),
    }
}

/// Stderr is diagnostic. Failing to write it must not replace the command's status.
fn write_quiet<E: Write>(writer: &Mutex<E>, text: &str) {
    let mut guard = writer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    drop(guard.write_all(text.as_bytes()));
    drop(guard.flush());
}

fn report<E: Write>(stderr: &Mutex<E>, failure: &Failure) {
    report_message(stderr, failure.message());
}

fn report_message<E: Write>(stderr: &Mutex<E>, message: &str) {
    let message = one_line(message);
    write_quiet(stderr, &format!("jev: {message}\n"));
}
