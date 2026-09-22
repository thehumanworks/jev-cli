//! Clap command definitions.
//!
//! Environment variables are not clap `env` attributes. Help must not change when a variable is
//! set, and tests inject an environment without touching the process. [`apply_env`] applies flag,
//! then environment, then the default, after a successful parse.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use clap::parser::ValueSource;
use clap::{ArgMatches, Args, ColorChoice, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use typesafe_jev::{DEFAULT_BASE_URL, DEFAULT_MODEL};

use crate::exit::Failure;

/// Inclusive ceiling for `--timeout` and `JEV_TIMEOUT`: one day, in seconds.
const MAX_TIMEOUT_SECS: u64 = 86_400;

const AFTER_HELP: &str = "\
exit codes:
  0  success (noul --threshold: the probability is at least P)
  1  noul --threshold: the probability is below P
  2  usage error (arguments, state, or questions document)
  3  authentication: no API key, or the API rejected it
  4  the API refused the request, or the configuration is unusable
  5  the request exceeds the model's context
  6  other API or network failure, including retries exhausted
  7  local I/O failure (unreadable file or stdin)

examples:
  jev noul \"Is this spam?\" -s \"You have won a prize\"
  jev noul \"Is this spam?\" -f mail.txt --threshold 0.8 && echo spam
  jev choice \"Which team?\" billing=Payments technical=Bugs -f ticket.txt
  jev score \"How urgent?\" \"Not urgent\" Urgent -s \"Fix this today\"
  jev example | jev ask -q - -s \"Please help ASAP\"
  jev ask -q questions.json -f ticket.txt --json
";

/// Ask calibrated questions about a state with the Jev model.
#[derive(Parser)]
#[command(
    name = "jev",
    version,
    color = ColorChoice::Never,
    term_width = 100,
    args_override_self = true,
    subcommand_required = true,
    arg_required_else_help = true,
    propagate_version = true,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// What the user asked for, plus which connection flags were typed rather than defaulted.
pub(crate) struct Parsed {
    /// The selected subcommand.
    pub command: Command,
    /// Flags the user actually passed. Defaults and the environment are not "explicit".
    pub explicit: Explicit,
}

/// Which connection and output flags were present on the command line.
#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent flag, not a mode
pub(crate) struct Explicit {
    /// `--api-key` was present.
    pub api_key: bool,
    /// `--base-url` was present.
    pub base_url: bool,
    /// `--model` was present.
    pub model: bool,
    /// `--timeout` was present.
    pub timeout: bool,
    /// `--retries` was present.
    pub retries: bool,
    /// `--output` was present.
    pub output: bool,
}

impl Explicit {
    /// No flag was typed.
    pub(crate) fn none() -> Self {
        Self { api_key: false, base_url: false, model: false, timeout: false, retries: false, output: false }
    }

    fn from_matches(matches: &ArgMatches) -> Self {
        let Some((_, sub)) = matches.subcommand() else {
            return Self::none();
        };
        Self {
            api_key: is_command_line(sub, "api_key"),
            base_url: is_command_line(sub, "base_url"),
            model: is_command_line(sub, "model"),
            timeout: is_command_line(sub, "timeout"),
            retries: is_command_line(sub, "retries"),
            output: is_command_line(sub, "output"),
        }
    }
}

fn is_command_line(matches: &ArgMatches, id: &str) -> bool {
    // `value_source` panics when the subcommand does not have the flag (`example`, `completions`).
    let known = matches.ids().any(|candidate| candidate.as_str() == id);
    known && matches.value_source(id) == Some(ValueSource::CommandLine)
}

/// `text` or `json`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Two-decimal blocks for `ask`, or one bare value for a single question.
    #[default]
    Text,
    /// The response document plus `cost_usd`.
    Json,
}

/// Subcommands.
#[derive(Subcommand)]
pub(crate) enum Command {
    /// Ask one or more questions from a JSON questions document
    #[command(args_override_self = true)]
    Ask(AskArgs),
    /// Ask one yes/no question
    #[command(args_override_self = true)]
    Noul(NoulArgs),
    /// Ask one multiple-choice question
    #[command(args_override_self = true)]
    Choice(ChoiceArgs),
    /// Ask one rating question
    #[command(args_override_self = true)]
    Score(ScoreArgs),
    /// Print an example questions document
    Example(ExampleArgs),
    /// Print a shell completion script
    Completions(CompletionsArgs),
}

/// `jev ask`.
#[derive(Args)]
pub(crate) struct AskArgs {
    /// Questions document (`-` means stdin)
    #[arg(short = 'q', long = "questions", value_name = "PATH")]
    pub questions: Option<PathBuf>,
    /// Questions document as inline JSON
    #[arg(long = "questions-json", value_name = "JSON", conflicts_with = "questions")]
    pub questions_json: Option<String>,
    #[command(flatten)]
    pub common: CommonArgs,
}

/// `jev noul`.
#[derive(Args)]
pub(crate) struct NoulArgs {
    /// The yes/no question
    #[arg(value_name = "INSTRUCTIONS", allow_hyphen_values = true)]
    pub instructions: String,
    /// What a yes (a probability near 1) means
    #[arg(long, value_name = "MEANING", allow_hyphen_values = true)]
    pub yes: Option<String>,
    /// What a no (a probability near 0) means
    #[arg(long, value_name = "MEANING", allow_hyphen_values = true)]
    pub no: Option<String>,
    /// Question id
    #[arg(long, value_name = "ID", default_value = "q")]
    pub id: String,
    /// Exit 0 when the yes-probability is at least P, otherwise exit 1. P is between 0 and 1
    #[arg(long, value_name = "P", allow_hyphen_values = true)]
    pub threshold: Option<f64>,
    #[command(flatten)]
    pub common: CommonArgs,
}

/// `jev choice`.
#[derive(Args)]
pub(crate) struct ChoiceArgs {
    /// What the model should decide
    #[arg(value_name = "INSTRUCTIONS", allow_hyphen_values = true)]
    pub instructions: String,
    /// Option as `name` or `name=description` (2 to 255; names unique and non-empty). `--` must come after every flag
    #[arg(value_name = "OPTION")]
    pub options: Vec<String>,
    /// Question id
    #[arg(long, value_name = "ID", default_value = "q")]
    pub id: String,
    #[command(flatten)]
    pub common: CommonArgs,
}

/// `jev score`.
#[derive(Args)]
pub(crate) struct ScoreArgs {
    /// What the model should rate
    #[arg(value_name = "INSTRUCTIONS", allow_hyphen_values = true)]
    pub instructions: String,
    /// Level description, low end of the scale first (2 to 10 levels). `--` must come after every flag
    #[arg(value_name = "LEVEL")]
    pub levels: Vec<String>,
    /// Question id
    #[arg(long, value_name = "ID", default_value = "q")]
    pub id: String,
    #[command(flatten)]
    pub common: CommonArgs,
}

/// `jev example`.
#[derive(Args)]
pub(crate) struct ExampleArgs {
    /// Print an example state instead of the questions document
    #[arg(long)]
    pub state: bool,
}

/// `jev completions`.
#[derive(Args)]
pub(crate) struct CompletionsArgs {
    /// Shell to generate a completion script for
    #[arg(value_enum, value_name = "SHELL")]
    pub shell: Shell,
}

/// Flags shared by the commands that send a request.
#[derive(Args)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent flag, not a mode
pub(crate) struct CommonArgs {
    /// State text
    #[arg(short = 's', long = "state", value_name = "TEXT", conflicts_with = "state_file", help_heading = "State")]
    pub state: Option<String>,
    /// Read the state from a file (`-` means stdin)
    #[arg(short = 'f', long = "state-file", value_name = "PATH", conflicts_with = "state", help_heading = "State")]
    pub state_file: Option<PathBuf>,
    /// Parse the state as a JSON object, array or string
    #[arg(long = "state-json", help_heading = "State")]
    pub state_json: bool,
    /// Output format
    #[arg(
        short = 'o',
        long = "output",
        value_enum,
        value_name = "FORMAT",
        default_value = "text",
        help_heading = "Output"
    )]
    pub output: OutputFormat,
    /// Shorthand for `--output json`
    #[arg(long = "json", help_heading = "Output")]
    pub json: bool,
    /// After a successful request, print model, tokens, retries and estimated cost on stderr
    #[arg(short = 'v', long = "verbose", help_heading = "Output")]
    pub verbose: bool,
    /// Print each failed attempt on stderr as it happens
    #[arg(long = "debug", help_heading = "Output")]
    pub debug: bool,
    /// Print the request JSON and do not send it. No API key is required
    #[arg(long = "dry-run", help_heading = "Output")]
    pub dry_run: bool,
    /// API key (`TYPESAFE_API_KEY`). Prefer the environment variable: flags are visible to other processes
    #[arg(long = "api-key", value_name = "KEY", help_heading = "Connection")]
    pub api_key: Option<String>,
    /// API endpoint (`JEV_BASE_URL`)
    #[arg(long = "base-url", value_name = "URL", default_value = DEFAULT_BASE_URL, help_heading = "Connection")]
    pub base_url: String,
    /// Model name (`JEV_MODEL`)
    #[arg(long = "model", value_name = "NAME", default_value = DEFAULT_MODEL, help_heading = "Connection")]
    pub model: String,
    /// Per-request timeout in whole seconds (`JEV_TIMEOUT`), 1 to 86400. Also bounds connecting
    #[arg(
        long = "timeout",
        value_name = "SECONDS",
        value_parser = clap::value_parser!(u64).range(1..=MAX_TIMEOUT_SECS),
        default_value_t = 60,
        help_heading = "Connection"
    )]
    pub timeout: u64,
    /// Extra attempts after the first for transient failures (`JEV_RETRIES`)
    #[arg(long = "retries", value_name = "N", default_value_t = 8, help_heading = "Connection")]
    pub retries: u32,
}

/// Parse `args`, which include the program name. Help and version come back as a [`clap::Error`].
pub(crate) fn parse(args: &[OsString]) -> Result<Parsed, clap::Error> {
    let matches = Cli::command().try_get_matches_from(args)?;
    let cli = Cli::from_arg_matches(&matches)?;
    Ok(Parsed { command: cli.command, explicit: Explicit::from_matches(&matches) })
}

/// The clap command, used for `--help` rendering and shell completions.
pub(crate) fn command() -> clap::Command {
    Cli::command()
}

/// Borrow the shared flags, when this subcommand has them.
pub(crate) fn common_mut(command: &mut Command) -> Option<&mut CommonArgs> {
    match command {
        Command::Ask(args) => Some(&mut args.common),
        Command::Noul(args) => Some(&mut args.common),
        Command::Choice(args) => Some(&mut args.common),
        Command::Score(args) => Some(&mut args.common),
        Command::Example(_) | Command::Completions(_) => None,
    }
}

/// Fill defaults from `env` when the flag was not on the command line.
///
/// An empty variable is ignored. A non-numeric timeout or retry count is a usage error.
pub(crate) fn apply_env(
    common: &mut CommonArgs,
    explicit: Explicit,
    env: Option<&BTreeMap<String, String>>,
) -> Result<(), Failure> {
    if !explicit.api_key
        && let Some(value) = env_value(env, "TYPESAFE_API_KEY")
    {
        common.api_key = Some(value);
    }
    if !explicit.base_url
        && let Some(value) = env_value(env, "JEV_BASE_URL")
    {
        common.base_url = value;
    }
    if !explicit.model
        && let Some(value) = env_value(env, "JEV_MODEL")
    {
        common.model = value;
    }
    if !explicit.timeout
        && let Some(value) = env_value(env, "JEV_TIMEOUT")
    {
        let seconds: u64 = parse_whole(&value, "JEV_TIMEOUT must be a whole number of seconds")?;
        if !(1..=MAX_TIMEOUT_SECS).contains(&seconds) {
            return Err(Failure::Usage(format!("{seconds} is not in 1..={MAX_TIMEOUT_SECS}")));
        }
        common.timeout = seconds;
    }
    if !explicit.retries
        && let Some(value) = env_value(env, "JEV_RETRIES")
    {
        common.retries = parse_whole(&value, "JEV_RETRIES must be a whole number")?;
    }
    Ok(())
}

/// `--json` forces JSON. Combining it with `--output text` is a usage error.
pub(crate) fn output_format(common: &CommonArgs, explicit: Explicit) -> Result<OutputFormat, Failure> {
    if common.json && explicit.output && common.output != OutputFormat::Json {
        Err(Failure::Usage("--json cannot be combined with --output text".into()))
    } else if common.json {
        Ok(OutputFormat::Json)
    } else {
        Ok(common.output)
    }
}

fn env_value(env: Option<&BTreeMap<String, String>>, name: &str) -> Option<String> {
    let value = match env {
        Some(map) => map.get(name).cloned(),
        None => std::env::var(name).ok(),
    }?;
    if value.is_empty() { None } else { Some(value) }
}

fn parse_whole<T: std::str::FromStr>(value: &str, message: &str) -> Result<T, Failure> {
    value.parse::<T>().map_err(|_| Failure::Usage(format!("{message}, not {value:?}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::{CommonArgs, Explicit, OutputFormat, apply_env, output_format};
    use typesafe_jev::{DEFAULT_BASE_URL, DEFAULT_MODEL};

    fn defaults() -> CommonArgs {
        CommonArgs {
            state: None,
            state_file: None,
            state_json: false,
            output: OutputFormat::Text,
            json: false,
            verbose: false,
            debug: false,
            dry_run: false,
            api_key: None,
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            timeout: 60,
            retries: 8,
        }
    }

    #[test]
    fn environment_fills_blanks_and_flags_win() {
        let mut env = BTreeMap::new();
        env.insert("TYPESAFE_API_KEY".into(), "from-env-key".into());
        env.insert("JEV_BASE_URL".into(), "http://127.0.0.1:9".into());
        env.insert("JEV_MODEL".into(), "from-env".into());
        env.insert("JEV_TIMEOUT".into(), "15".into());
        env.insert("JEV_RETRIES".into(), "3".into());
        env.insert("IGNORED".into(), String::new());

        let mut common = defaults();
        apply_env(&mut common, Explicit::none(), Some(&env)).unwrap();
        assert_eq!(common.api_key.as_deref(), Some("from-env-key"));
        assert_eq!(common.base_url, "http://127.0.0.1:9");
        assert_eq!(common.model, "from-env");
        assert_eq!(common.timeout, 15);
        assert_eq!(common.retries, 3);

        let mut flagged = defaults();
        flagged.model = "from-flag".into();
        let mut explicit = Explicit::none();
        explicit.model = true;
        apply_env(&mut flagged, explicit, Some(&env)).unwrap();
        assert_eq!(flagged.model, "from-flag");
    }

    #[test]
    fn empty_environment_values_are_ignored_and_bad_numbers_are_usage() {
        let mut env = BTreeMap::new();
        env.insert("JEV_MODEL".into(), String::new());
        env.insert("JEV_TIMEOUT".into(), String::new());
        let mut common = defaults();
        apply_env(&mut common, Explicit::none(), Some(&env)).unwrap();
        assert_eq!(common.model, DEFAULT_MODEL);
        assert_eq!(common.timeout, 60);

        let mut bad = BTreeMap::new();
        bad.insert("JEV_TIMEOUT".into(), "soon".into());
        let err = apply_env(&mut defaults(), Explicit::none(), Some(&bad)).unwrap_err();
        assert_eq!(err.code(), 2);
        assert!(err.message().contains("JEV_TIMEOUT"), "{}", err.message());

        let mut zero = BTreeMap::new();
        zero.insert("JEV_TIMEOUT".into(), "0".into());
        let err = apply_env(&mut defaults(), Explicit::none(), Some(&zero)).unwrap_err();
        assert_eq!(err.message(), "0 is not in 1..=86400");

        let mut huge = BTreeMap::new();
        huge.insert("JEV_TIMEOUT".into(), "86401".into());
        let err = apply_env(&mut defaults(), Explicit::none(), Some(&huge)).unwrap_err();
        assert_eq!(err.message(), "86401 is not in 1..=86400");

        let mut bad = BTreeMap::new();
        bad.insert("JEV_RETRIES".into(), "-1".into());
        let err = apply_env(&mut defaults(), Explicit::none(), Some(&bad)).unwrap_err();
        assert!(err.message().contains("JEV_RETRIES"), "{}", err.message());
    }

    #[test]
    fn json_shorthand_agrees_with_output_or_replaces_the_default() {
        let mut common = defaults();
        common.json = true;
        assert_eq!(output_format(&common, Explicit::none()).unwrap(), OutputFormat::Json);

        common.output = OutputFormat::Json;
        let mut explicit = Explicit::none();
        explicit.output = true;
        assert_eq!(output_format(&common, explicit).unwrap(), OutputFormat::Json);

        common.output = OutputFormat::Text;
        let err = output_format(&common, explicit).unwrap_err();
        assert!(err.message().contains("--output text"));
    }

    #[test]
    fn defaults_match_the_crate() {
        let config = typesafe_jev::Config::default();
        let common = defaults();
        assert_eq!(common.base_url, config.base_url);
        assert_eq!(common.model, config.model);
        assert_eq!(common.timeout, config.timeout.as_secs());
        assert_eq!(common.retries, config.max_retries);
        assert_eq!(PathBuf::from("-").as_os_str(), "-");
    }
}
