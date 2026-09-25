//! Presets: the crate's questions, a first-match rule table over their answers, and the actions
//! the rules choose between.
//!
//! `questions` is the crate's `Questions` document, read with the same rules as `ask -q`. The rest
//! is this program's own format. It denies unknown fields, so a misspelt `at_leats` is an error
//! rather than a condition that quietly never holds. A repeated key is an error too: serde's
//! last-wins rule would hide the first entry. Everything is checked before any request is sent.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde_json::{Value, json};
use typesafe_jev::{Question, Questions};

use crate::cli::env_value;
use crate::exit::Failure;
use crate::questions::{deserialize_questions, example_questions};
use crate::state::strip_bom;

/// A preset that passed every check `decide` needs.
pub(crate) struct Preset {
    /// Sent as the request's questions, unchanged.
    pub(crate) questions: Questions,
    /// In order. The first rule whose conditions all hold decides.
    pub(crate) rules: Vec<Rule>,
    /// The action when no rule matches.
    pub(crate) fallback: Option<String>,
    /// Every action, in the order the preset lists them.
    pub(crate) actions: Vec<(String, Action)>,
}

/// One row of the rule table.
pub(crate) struct Rule {
    /// All must hold. An empty list always holds.
    pub(crate) conditions: Vec<Condition>,
    /// What the rule chooses.
    pub(crate) then: Then,
}

/// One test against one answer.
pub(crate) struct Condition {
    /// The question id.
    pub(crate) question: String,
    /// The test.
    pub(crate) test: Test,
    /// What a unit of margin means: `levels - 1` for a score, 1 otherwise.
    pub(crate) width: f64,
}

/// A test on one answer. Which answers a test applies to is checked when the preset is read.
pub(crate) enum Test {
    /// A noul's probability, or a score's position, is at least this.
    AtLeast(f64),
    /// A noul's probability, or a score's position, is below this.
    Below(f64),
    /// A choice chose this option.
    Is(String),
    /// A choice chose one of these options.
    In(Vec<String>),
    /// A choice's or a score's confidence is at least this.
    ConfidenceAtLeast(f64),
}

impl Test {
    /// The key the preset used, which is also the `test` in the JSON output.
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::AtLeast(_) => "at_least",
            Self::Below(_) => "below",
            Self::Is(_) => "is",
            Self::In(_) => "in",
            Self::ConfidenceAtLeast(_) => "confidence_at_least",
        }
    }
}

/// What a matching rule chooses.
pub(crate) enum Then {
    /// This action.
    Action(String),
    /// The action named by this choice's answer.
    From(String),
}

/// An outcome of the preset.
pub(crate) struct Action {
    /// The program and its arguments, for `call`. Never passed through a shell.
    pub(crate) run: Option<Vec<String>>,
}

impl Preset {
    /// The action named `name`.
    pub(crate) fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|(candidate, _)| candidate == name).map(|(_, action)| action)
    }

    /// The extra checks `call` needs: a fallback, and a program for every action.
    pub(crate) fn check_callable(&self) -> Result<(), Failure> {
        if self.fallback.is_none() {
            return Err(Failure::Usage("call needs a fallback action".into()));
        }
        if let Some((name, _)) = self.actions.iter().find(|(_, action)| action.run.is_none()) {
            return Err(Failure::Usage(format!("call needs a run for every action, and `{name}` has none")));
        }
        Ok(())
    }
}

/// Parse and check a preset. The message does not say which preset: the caller adds that.
pub(crate) fn parse_preset(text: &str) -> Result<Preset, Failure> {
    let text = strip_bom(text);
    if text.trim().is_empty() {
        return Err(Failure::Usage("the preset is empty".into()));
    }
    let raw: RawPreset = serde_json::from_str(text).map_err(|err| Failure::Usage(err.to_string()))?;
    check(raw).map_err(Failure::Usage)
}

/// Where a preset comes from.
#[derive(Debug)]
pub(crate) enum Source {
    /// Standard input.
    Stdin,
    /// A file, given as a path or found by name.
    File(PathBuf),
}

impl Source {
    /// How the preset is named in messages and in `JEV_PRESET`.
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Stdin => "-".to_owned(),
            Self::File(path) => path.display().to_string(),
        }
    }

    /// The directory a relative program path is resolved against. `None` is the working directory.
    pub(crate) fn directory(&self) -> Option<&Path> {
        match self {
            Self::Stdin => None,
            Self::File(path) => path.parent(),
        }
    }
}

/// Turn the `PRESET` argument into a source.
///
/// `-` is stdin. A value that contains `/` or ends in `.json` is a path. Anything else is a name,
/// looked up as `.jev/presets/NAME.json` from `cwd` upwards, then under the user's configuration
/// directory (`XDG_CONFIG_HOME`, else `HOME/.config`) as `jev/presets/NAME.json`. `cwd` is only
/// called for a name.
pub(crate) fn locate(
    value: &str,
    cwd: impl FnOnce() -> std::io::Result<PathBuf>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<Source, Failure> {
    if value == "-" {
        return Ok(Source::Stdin);
    }
    if value.is_empty() {
        return Err(Failure::Usage("the preset name is empty".into()));
    }
    if value.contains('/')
        || Path::new(value).extension().is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Ok(Source::File(PathBuf::from(value)));
    }
    let file = format!("{value}.json");
    let cwd = cwd().map_err(|err| Failure::Io(format!("cannot read the working directory: {err}")))?;
    for dir in cwd.ancestors() {
        let candidate = dir.join(".jev").join("presets").join(&file);
        if candidate.is_file() {
            return Ok(Source::File(candidate));
        }
    }
    let config = config_home(env).map(|home| home.join("jev").join("presets"));
    if let Some(candidate) = config.as_ref().map(|dir| dir.join(&file))
        && candidate.is_file()
    {
        return Ok(Source::File(candidate));
    }
    let places = match &config {
        Some(dir) => format!(".jev/presets or {}", dir.display()),
        None => ".jev/presets".to_owned(),
    };
    Err(Failure::Usage(format!("no preset named `{value}` in {places}")))
}

fn config_home(env: Option<&BTreeMap<String, String>>) -> Option<PathBuf> {
    if let Some(dir) = env_value(env, "XDG_CONFIG_HOME") {
        return Some(PathBuf::from(dir));
    }
    env_value(env, "HOME").map(|home| Path::new(&home).join(".config"))
}

/// A preset over the example questions, for `jev example --preset`. The actions only echo.
pub(crate) fn example_preset() -> Result<Value, Failure> {
    let questions = serde_json::to_value(example_questions())
        .map_err(|err| Failure::Api(format!("cannot format the example: {err}")))?;
    Ok(json!({
        "questions": questions,
        "rules": [
            {
                "when": { "is_urgent": { "at_least": 0.8 }, "frustration": { "at_least": 1.5 } },
                "then": "page"
            },
            { "when": { "department": { "confidence_at_least": 0.6 } }, "then": { "from": "department" } }
        ],
        "fallback": "triage",
        "actions": {
            "billing": { "run": ["echo", "billing queue"] },
            "technical": { "run": ["echo", "technical queue"] },
            "sales": { "run": ["echo", "sales queue"] },
            "page": { "run": ["echo", "page the on-call engineer"] },
            "triage": { "run": ["echo", "triage queue"] }
        }
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPreset {
    #[serde(deserialize_with = "deserialize_questions")]
    questions: Questions,
    rules: Vec<RawRule>,
    #[serde(default)]
    fallback: Option<String>,
    actions: Entries<RawAction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    #[serde(default)]
    when: Entries<RawCondition>,
    then: RawThen,
}

#[derive(Deserialize)]
#[serde(untagged, expecting = "an action name, or {\"from\": \"<choice id>\"}")]
enum RawThen {
    Action(String),
    From(FromChoice),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FromChoice {
    from: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondition {
    at_least: Option<f64>,
    below: Option<f64>,
    is: Option<String>,
    #[serde(rename = "in")]
    one_of: Option<Vec<String>>,
    confidence_at_least: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    #[serde(default)]
    run: Option<Vec<String>>,
}

/// A JSON object read in order. A repeated key is an error.
struct Entries<V>(Vec<(String, V)>);

impl<V> Default for Entries<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Entries<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(EntriesVisitor(PhantomData))
    }
}

struct EntriesVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for EntriesVisitor<V> {
    type Value = Entries<V>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("the key \"{key}\" appears twice")));
            }
            entries.push((key, map.next_value()?));
        }
        Ok(Entries(entries))
    }
}

/// What a condition can be tested against.
enum Kind<'a> {
    Noul,
    Choice(Vec<&'a str>),
    Score(usize),
}

fn kind_of(question: &Question) -> Option<Kind<'_>> {
    match question {
        Question::Noul(_) => Some(Kind::Noul),
        Question::Choice(choice) => Some(Kind::Choice(choice.criteria.keys().map(String::as_str).collect())),
        Question::Score(score) => Some(Kind::Score(score.criteria.len())),
        // `Question` is `non_exhaustive`.
        #[allow(clippy::wildcard_enum_match_arm)]
        _ => None,
    }
}

fn check(raw: RawPreset) -> Result<Preset, String> {
    if raw.questions.is_empty() {
        return Err("there are no questions".into());
    }
    if raw.rules.is_empty() {
        return Err("there are no rules".into());
    }
    if raw.actions.0.is_empty() {
        return Err("there are no actions".into());
    }
    let mut kinds = HashMap::new();
    for (id, question) in &raw.questions {
        let kind = kind_of(question).ok_or_else(|| format!("`{id}` is a question type this version cannot read"))?;
        kinds.insert(id.as_str(), kind);
    }
    let mut actions = Vec::with_capacity(raw.actions.0.len());
    for (name, action) in raw.actions.0 {
        if name.is_empty() {
            return Err("an action name is empty".into());
        }
        match action.run.as_deref() {
            Some([]) => return Err(format!("action `{name}`: run is empty")),
            Some([program, ..]) if program.is_empty() => return Err(format!("action `{name}`: the program is empty")),
            _ => {}
        }
        actions.push((name, Action { run: action.run }));
    }
    let names: HashSet<&str> = actions.iter().map(|(name, _)| name.as_str()).collect();
    let mut rules = Vec::with_capacity(raw.rules.len());
    for (index, rule) in raw.rules.into_iter().enumerate() {
        let number = index + 1;
        let mut conditions = Vec::new();
        for (id, condition) in rule.when.0 {
            let kind = kinds.get(id.as_str()).ok_or_else(|| format!("rule {number}: `{id}` is not a question"))?;
            let tests = tests(&id, condition, kind).map_err(|message| format!("rule {number}: {id}: {message}"))?;
            let width = match kind {
                Kind::Score(levels) => f64::from(u32::try_from(levels.saturating_sub(1)).unwrap_or(u32::MAX).max(1)),
                Kind::Noul | Kind::Choice(_) => 1.0,
            };
            conditions.extend(tests.into_iter().map(|test| Condition { question: id.clone(), test, width }));
        }
        let then = match rule.then {
            RawThen::Action(name) if names.contains(name.as_str()) => Then::Action(name),
            RawThen::Action(name) => return Err(format!("rule {number}: `{name}` is not an action")),
            RawThen::From(FromChoice { from }) => {
                let options = match kinds.get(from.as_str()) {
                    None => return Err(format!("rule {number}: from: `{from}` is not a question")),
                    Some(Kind::Choice(options)) => options,
                    Some(Kind::Noul | Kind::Score(_)) => {
                        return Err(format!("rule {number}: from: `{from}` is not a choice"));
                    }
                };
                if let Some(option) = options.iter().find(|option| !names.contains(**option)) {
                    return Err(format!("rule {number}: from: option `{option}` of `{from}` is not an action"));
                }
                Then::From(from)
            }
        };
        rules.push(Rule { conditions, then });
    }
    if let Some(fallback) = &raw.fallback
        && !names.contains(fallback.as_str())
    {
        return Err(format!("fallback: `{fallback}` is not an action"));
    }
    Ok(Preset { questions: raw.questions, rules, fallback: raw.fallback, actions })
}

/// The tests of one condition object, in a fixed order: `at_least`, `below`, `is`, `in`,
/// `confidence_at_least`.
fn tests(id: &str, condition: RawCondition, kind: &Kind<'_>) -> Result<Vec<Test>, String> {
    let RawCondition { at_least, below, is, one_of, confidence_at_least } = condition;
    if at_least.is_none() && below.is_none() && is.is_none() && one_of.is_none() && confidence_at_least.is_none() {
        return Err("the condition is empty".into());
    }
    let top = match kind {
        Kind::Noul => {
            if is.is_some() || one_of.is_some() || confidence_at_least.is_some() {
                return Err("a noul takes at_least and below".into());
            }
            1.0
        }
        Kind::Score(levels) => {
            if is.is_some() || one_of.is_some() {
                return Err("a score takes at_least, below and confidence_at_least".into());
            }
            f64::from(u32::try_from(levels.saturating_sub(1)).unwrap_or(u32::MAX))
        }
        Kind::Choice(options) => {
            if at_least.is_some() || below.is_some() {
                return Err("a choice takes is, in and confidence_at_least".into());
            }
            if is.is_some() && one_of.is_some() {
                return Err("use is or in, not both".into());
            }
            let named = is.iter().chain(one_of.iter().flatten());
            if let Some(unknown) = named.clone().find(|name| !options.contains(&name.as_str())) {
                return Err(format!("`{unknown}` is not an option of `{id}`"));
            }
            if one_of.as_ref().is_some_and(Vec::is_empty) {
                return Err("in is empty".into());
            }
            1.0
        }
    };
    for (key, value) in [("at_least", at_least), ("below", below)] {
        if let Some(value) = value
            && !(0.0..=top).contains(&value)
        {
            return Err(format!("{key} must be between 0 and {top}"));
        }
    }
    if let Some(value) = confidence_at_least
        && !(0.0..=1.0).contains(&value)
    {
        return Err("confidence_at_least must be between 0 and 1".into());
    }
    if let (Some(low), Some(high)) = (at_least, below)
        && low >= high
    {
        return Err("at_least must be less than below".into());
    }
    let mut tests = Vec::new();
    tests.extend(at_least.map(Test::AtLeast));
    tests.extend(below.map(Test::Below));
    tests.extend(is.map(Test::Is));
    tests.extend(one_of.map(Test::In));
    tests.extend(confidence_at_least.map(Test::ConfidenceAtLeast));
    Ok(tests)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeMap;
    use std::fs;

    use super::{Source, Test, Then, example_preset, locate, parse_preset};

    /// One question of each type, a guard, an AND, a `from`, and a fallback.
    const CI: &str = include_str!("../tests/fixtures/presets/ci.json");

    /// `CI` with one JSON-pointer location replaced.
    fn edit(pointer: &str, value: serde_json::Value) -> String {
        let mut preset: serde_json::Value = serde_json::from_str(CI).unwrap();
        *preset.pointer_mut(pointer).unwrap() = value;
        preset.to_string()
    }

    fn error(text: &str) -> String {
        match parse_preset(text) {
            Ok(_) => panic!("accepted: {text}"),
            Err(failure) => {
                assert_eq!(failure.code(), 2);
                failure.message().to_owned()
            }
        }
    }

    #[test]
    fn the_ci_preset_reads_in_order() {
        let preset = parse_preset(CI).unwrap();
        assert_eq!(preset.questions.ids().collect::<Vec<_>>(), ["secrets", "kind", "scope"]);
        assert_eq!(preset.rules.len(), 3);
        let second: Vec<_> = preset.rules[1].conditions.iter().map(|c| (c.question.as_str(), c.test.key())).collect();
        assert_eq!(second, [("kind", "is"), ("scope", "at_least")]);
        assert!((preset.rules[1].conditions[1].width - 2.0).abs() < f64::EPSILON);
        assert!(matches!(&preset.rules[2].then, Then::From(id) if id == "kind"));
        assert_eq!(preset.fallback.as_deref(), Some("escalate"));
        assert_eq!(
            preset.actions.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
            ["retry", "fmt", "issue", "escalate"]
        );
        assert!(preset.check_callable().is_ok());
    }

    #[test]
    fn a_condition_object_may_combine_tests_of_its_type() {
        let text = edit("/rules/1/when/kind", serde_json::json!({"is": "issue", "confidence_at_least": 0.7}));
        let preset = parse_preset(&text).unwrap();
        let tests: Vec<_> = preset.rules[1].conditions.iter().map(|c| c.test.key()).collect();
        assert_eq!(tests, ["is", "confidence_at_least", "at_least"]);
        let band = edit("/rules/0/when/secrets", serde_json::json!({"at_least": 0.3, "below": 0.8}));
        let preset = parse_preset(&band).unwrap();
        assert!(matches!(preset.rules[0].conditions[1].test, Test::Below(p) if (p - 0.8).abs() < f64::EPSILON));
    }

    #[test]
    fn an_empty_when_always_holds_and_fallback_is_optional() {
        let mut preset: serde_json::Value = serde_json::from_str(CI).unwrap();
        preset["rules"] = serde_json::json!([{ "then": { "from": "kind" } }]);
        preset.as_object_mut().unwrap().remove("fallback");
        let parsed = parse_preset(&preset.to_string()).unwrap();
        assert!(parsed.rules[0].conditions.is_empty());
        assert_eq!(parsed.fallback, None);
        assert_eq!(parsed.check_callable().unwrap_err().message(), "call needs a fallback action");
    }

    #[test]
    fn every_check_names_the_rule_and_the_field() {
        use serde_json::json;
        let cases = [
            (edit("/rules/0/when", json!({"secret": {"at_least": 0.3}})), "rule 1: `secret` is not a question"),
            (edit("/rules/0/when/secrets", json!({})), "rule 1: secrets: the condition is empty"),
            (edit("/rules/0/when/secrets", json!({"is": "yes"})), "rule 1: secrets: a noul takes at_least and below"),
            (
                edit("/rules/0/when/secrets", json!({"confidence_at_least": 0.5})),
                "rule 1: secrets: a noul takes at_least and below",
            ),
            (
                edit("/rules/0/when/secrets", json!({"at_least": 1.5})),
                "rule 1: secrets: at_least must be between 0 and 1",
            ),
            (edit("/rules/0/when/secrets", json!({"below": -0.1})), "rule 1: secrets: below must be between 0 and 1"),
            (
                edit("/rules/0/when/secrets", json!({"at_least": 0.8, "below": 0.3})),
                "rule 1: secrets: at_least must be less than below",
            ),
            (edit("/rules/1/when/scope", json!({"at_least": 2.5})), "rule 2: scope: at_least must be between 0 and 2"),
            (
                edit("/rules/1/when/scope", json!({"is": "x"})),
                "rule 2: scope: a score takes at_least, below and confidence_at_least",
            ),
            (
                edit("/rules/1/when/scope", json!({"confidence_at_least": 1.2})),
                "rule 2: scope: confidence_at_least must be between 0 and 1",
            ),
            (
                edit("/rules/1/when/kind", json!({"at_least": 0.5})),
                "rule 2: kind: a choice takes is, in and confidence_at_least",
            ),
            (edit("/rules/1/when/kind", json!({"is": "issues"})), "rule 2: kind: `issues` is not an option of `kind`"),
            (
                edit("/rules/1/when/kind", json!({"in": ["fmt", "lint"]})),
                "rule 2: kind: `lint` is not an option of `kind`",
            ),
            (edit("/rules/1/when/kind", json!({"in": []})), "rule 2: kind: in is empty"),
            (edit("/rules/1/when/kind", json!({"is": "fmt", "in": ["fmt"]})), "rule 2: kind: use is or in, not both"),
            (edit("/rules/0/then", json!("page")), "rule 1: `page` is not an action"),
            (edit("/rules/2/then", json!({"from": "kinds"})), "rule 3: from: `kinds` is not a question"),
            (edit("/rules/2/then", json!({"from": "scope"})), "rule 3: from: `scope` is not a choice"),
            (
                edit("/actions", json!({"retry": {}, "issue": {}, "escalate": {}})),
                "rule 3: from: option `fmt` of `kind` is not an action",
            ),
            (edit("/fallback", json!("page")), "fallback: `page` is not an action"),
            (edit("/rules", json!([])), "there are no rules"),
            (edit("/actions", json!({})), "there are no actions"),
            (edit("/actions/retry", json!({"run": []})), "action `retry`: run is empty"),
            (edit("/actions/retry", json!({"run": [""]})), "action `retry`: the program is empty"),
            (String::new(), "the preset is empty"),
        ];
        for (text, expected) in cases {
            assert_eq!(error(&text), expected, "{text}");
        }
        let no_questions = edit("/questions", json!({}));
        assert_eq!(error(&no_questions), "there are no questions");
    }

    #[test]
    fn unknown_and_repeated_keys_are_serde_errors() {
        use serde_json::json;
        let misspelt = edit("/rules/0/when/secrets", json!({"at_leats": 0.3}));
        assert!(error(&misspelt).starts_with("unknown field `at_leats`"), "{}", error(&misspelt));
        let mut extra: serde_json::Value = serde_json::from_str(CI).unwrap();
        extra.as_object_mut().unwrap().insert("default".into(), json!("escalate"));
        assert!(error(&extra.to_string()).starts_with("unknown field `default`"), "{}", error(&extra.to_string()));
        let bad_then = edit("/rules/0/then", json!({"action": "escalate"}));
        assert!(error(&bad_then).starts_with("an action name, or {\"from\""), "{}", error(&bad_then));

        let rules = r#""rules": [{"then": "a"}]"#;
        let twice =
            format!(r#"{{"questions": {{"q": {{"type": "noul"}}}}, {rules}, "actions": {{"a": {{}}, "a": {{}}}}}}"#);
        assert!(error(&twice).starts_with("the key \"a\" appears twice"), "{}", error(&twice));
        let when_twice = r#"{"questions": {"q": {"type": "noul"}}, "rules": [{"when": {"q": {"below": 0.5}, "q": {"at_least": 0.5}}, "then": "a"}], "actions": {"a": {}}}"#;
        assert!(error(when_twice).starts_with("the key \"q\" appears twice"), "{}", error(when_twice));
        let question_twice = format!(
            r#"{{"questions": {{"q": {{"type": "noul"}}, "q": {{"type": "noul"}}}}, {rules}, "actions": {{"a": {{}}}}}}"#
        );
        assert!(error(&question_twice).contains("has the id \"q\" twice"), "{}", error(&question_twice));
    }

    #[test]
    fn call_needs_a_program_for_every_action() {
        let text = edit("/actions/fmt", serde_json::json!({}));
        let preset = parse_preset(&text).unwrap();
        assert_eq!(
            preset.check_callable().unwrap_err().message(),
            "call needs a run for every action, and `fmt` has none"
        );
    }

    #[test]
    fn the_example_preset_is_valid_and_callable() {
        let text = example_preset().unwrap().to_string();
        let preset = parse_preset(&text).unwrap();
        assert!(preset.check_callable().is_ok());
        assert_eq!(preset.questions.ids().collect::<Vec<_>>(), ["department", "frustration", "is_urgent"]);
    }

    #[test]
    fn the_git_preset_is_valid_and_callable() {
        let preset = parse_preset(include_str!("../.jev/presets/git.json")).unwrap();
        assert!(preset.check_callable().is_ok());
        assert_eq!(preset.questions.ids().collect::<Vec<_>>(), ["command", "destructive", "all_changes", "staged"]);
        assert!(
            matches!(preset.rules[0].conditions.as_slice(), [only] if only.question == "destructive"),
            "the guard is the first rule"
        );
        assert_eq!(preset.fallback.as_deref(), Some("unsure"));
    }

    #[test]
    fn names_are_found_upwards_then_in_the_config_directory() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let project = root.path().join(".jev").join("presets");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("triage.json"), CI).unwrap();
        let config = tempfile::tempdir().unwrap();
        let user = config.path().join("jev").join("presets");
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("triage.json"), CI).unwrap();
        fs::write(user.join("mine.json"), CI).unwrap();
        let mut env = BTreeMap::new();
        env.insert("XDG_CONFIG_HOME".to_owned(), config.path().display().to_string());

        let here = || Ok(nested.clone());
        let found = |name: &str| match locate(name, here, Some(&env)).unwrap() {
            Source::File(path) => path,
            Source::Stdin => panic!("stdin"),
        };
        assert_eq!(found("triage"), project.join("triage.json"), "the project wins over the user directory");
        assert_eq!(found("mine"), user.join("mine.json"));
        assert_eq!(found("x.json"), std::path::PathBuf::from("x.json"));
        assert_eq!(found("./x"), std::path::PathBuf::from("./x"));
        assert!(matches!(locate("-", here, Some(&env)).unwrap(), Source::Stdin));

        let missing = locate("nope", here, Some(&env)).unwrap_err();
        assert_eq!(missing.code(), 2);
        assert_eq!(
            missing.message(),
            format!("no preset named `nope` in .jev/presets or {}", user.display()),
            "the message names where it looked"
        );
        let mut home = BTreeMap::new();
        home.insert("HOME".to_owned(), config.path().display().to_string());
        let message = locate("nope", here, Some(&home)).unwrap_err().message().to_owned();
        assert!(message.ends_with(&format!("{}", config.path().join(".config").join("jev").join("presets").display())));
        assert_eq!(
            locate("nope", here, Some(&BTreeMap::new())).unwrap_err().message(),
            "no preset named `nope` in .jev/presets"
        );
        assert_eq!(locate("", here, None).unwrap_err().message(), "the preset name is empty");
        let gone = || Err(std::io::Error::other("gone"));
        assert!(matches!(locate("x.json", gone, None).unwrap(), Source::File(_)), "a path never reads the directory");
        assert_eq!(locate("x", gone, None).unwrap_err().code(), 7);
    }
}
