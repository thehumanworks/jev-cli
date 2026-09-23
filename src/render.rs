//! Text and JSON rendering.
//!
//! `ask` text is for a person reading a terminal: two decimal places, questions in the order they
//! were asked. The single-question commands print one bare value at full precision so a shell can
//! capture it. JSON is the crate's `Response` plus `cost_usd`, pretty-printed, and is the same
//! shape for every command. Choice lines follow the order the API returned; score levels run from
//! low to high. A level whose description is JSON is printed as compact JSON. `decide` prints the
//! action name, and its JSON puts the decision before the response keys. `--explain` is for a
//! person, on stderr.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::{Map, Number, Value};
use typesafe_jev::{Answer, Content, Question, Questions, Response};

use crate::decide::{ConditionCheck, Decision, Observed, RuleCheck, closest_call};
use crate::exit::Failure;
use crate::preset::{Preset, Test};
use crate::state::State;

/// Pretty JSON with a trailing newline.
pub(crate) fn pretty(value: &impl Serialize) -> Result<String, Failure> {
    let mut text =
        serde_json::to_string_pretty(value).map_err(|err| Failure::Api(format!("cannot format JSON: {err}")))?;
    text.push('\n');
    Ok(text)
}

/// The request body `Client::ask` would send: `model`, `state`, `questions`.
pub(crate) fn render_request(model: &str, state: &State, questions: &Questions) -> Result<String, Failure> {
    pretty(&RequestBody { model, state, questions })
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    state: &'a State,
    questions: &'a Questions,
}

/// `Response` as the crate serializes it, then `cost_usd` from [`typesafe_jev::Usage::cost_usd`].
pub(crate) fn render_response(response: &Response, cost_usd: f64) -> Result<String, Failure> {
    pretty(&Value::Object(response_object(response, cost_usd)?))
}

/// The `decide --json` document: `decision`, `rule`, `rules`, then the keys of [`render_response`].
pub(crate) fn decision_document(
    decision: &Decision<'_>,
    response: &Response,
    cost_usd: f64,
) -> Result<Map<String, Value>, Failure> {
    let mut document = Map::new();
    document.insert("decision".to_owned(), decision.action.clone().map_or(Value::Null, Value::String));
    document.insert("rule".to_owned(), decision.rule.map_or(Value::Null, Value::from));
    let rules = decision.rules.iter().map(rule_value).collect();
    document.insert("rules".to_owned(), Value::Array(rules));
    document.extend(response_object(response, cost_usd)?);
    Ok(document)
}

fn response_object(response: &Response, cost_usd: f64) -> Result<Map<String, Value>, Failure> {
    let value =
        serde_json::to_value(response).map_err(|err| Failure::Api(format!("cannot format the response: {err}")))?;
    let Value::Object(mut object) = value else {
        return Err(Failure::Api("cannot format the response".into()));
    };
    let number = Number::from_f64(cost_usd).unwrap_or_else(|| Number::from(0));
    object.insert("cost_usd".to_owned(), Value::Number(number));
    Ok(object)
}

fn rule_value(rule: &RuleCheck<'_>) -> Value {
    let conditions = rule.conditions.iter().map(condition_value).collect();
    let mut object = Map::new();
    object.insert("matched".to_owned(), Value::Bool(rule.matched));
    object.insert("conditions".to_owned(), Value::Array(conditions));
    Value::Object(object)
}

/// `question`, `test`, then `threshold`, `option` or `options`, then `value`, `met`, `margin`. A
/// non-finite number is `null`.
fn condition_value(checked: &ConditionCheck<'_>) -> Value {
    let condition = checked.condition;
    let mut object = Map::new();
    object.insert("question".to_owned(), Value::String(condition.question.clone()));
    object.insert("test".to_owned(), Value::String(condition.test.key().to_owned()));
    let (key, expected) = match &condition.test {
        Test::AtLeast(threshold) | Test::Below(threshold) | Test::ConfidenceAtLeast(threshold) => {
            ("threshold", Value::from(*threshold))
        }
        Test::Is(option) => ("option", Value::String(option.clone())),
        Test::In(options) => ("options", Value::from(options.clone())),
    };
    object.insert(key.to_owned(), expected);
    let value = match &checked.value {
        Observed::Number(number) => Value::from(*number),
        Observed::Option(option) => Value::String(option.clone()),
    };
    object.insert("value".to_owned(), value);
    object.insert("met".to_owned(), Value::Bool(checked.met));
    object.insert("margin".to_owned(), Value::from(checked.margin));
    Value::Object(object)
}

/// The `--explain` block for stderr: the decision, each rule with its values and margins, the
/// closest call, a blank line, then the answers in `ask`'s text format.
pub(crate) fn render_explain(preset: &Preset, decision: &Decision<'_>, response: &Response) -> Result<String, Failure> {
    let mut out = String::new();
    let _ = match (&decision.action, decision.rule) {
        (Some(action), Some(rule)) => writeln!(out, "decision: {action} (rule {rule})"),
        (Some(action), None) => writeln!(out, "decision: {action} (fallback)"),
        (None, _) => writeln!(out, "decision: none (no rule matched and there is no fallback)"),
    };
    let lines: Vec<Vec<String>> =
        decision.rules.iter().map(|rule| rule.conditions.iter().map(condition_text).collect()).collect();
    let label_width = format!("rule {}", decision.rules.len()).len();
    let text_width = lines.iter().flatten().map(String::len).max().unwrap_or(0);
    for (index, (rule, texts)) in decision.rules.iter().zip(&lines).enumerate() {
        let label = format!("rule {}", index + 1);
        let verdict = if rule.matched { "yes" } else { "no" };
        if texts.is_empty() {
            let _ = writeln!(out, "{label:<label_width$}  {verdict:<3}  always");
        }
        for (position, (checked, text)) in rule.conditions.iter().zip(texts).enumerate() {
            let (label, verdict) = if position == 0 { (label.as_str(), verdict) } else { ("", "") };
            let _ = writeln!(
                out,
                "{label:<label_width$}  {verdict:<3}  {text:<text_width$}  margin {:+.2}",
                checked.margin
            );
        }
    }
    if let Some((rule, checked)) = closest_call(decision) {
        let _ = writeln!(out, "closest call: rule {rule}, {} ({:+.2})", subject(checked), checked.margin);
    }
    out.push('\n');
    out.push_str(&render_ask_text(&preset.questions, response)?);
    Ok(out)
}

fn condition_text(checked: &ConditionCheck<'_>) -> String {
    let id = &checked.condition.question;
    let value = match &checked.value {
        Observed::Number(number) => format!("{number:.2}"),
        Observed::Option(option) => option.clone(),
    };
    match &checked.condition.test {
        Test::AtLeast(threshold) => format!("{id} {value} at_least {threshold:.2}"),
        Test::Below(threshold) => format!("{id} {value} below {threshold:.2}"),
        Test::ConfidenceAtLeast(threshold) => format!("{id} confidence {value} at_least {threshold:.2}"),
        Test::Is(option) => format!("{id} {value} is {option}"),
        Test::In(options) => format!("{id} {value} in [{}]", options.join(", ")),
    }
}

fn subject(checked: &ConditionCheck<'_>) -> String {
    let id = &checked.condition.question;
    match checked.condition.test {
        Test::ConfidenceAtLeast(_) => format!("{id} confidence"),
        Test::AtLeast(_) | Test::Below(_) | Test::Is(_) | Test::In(_) => id.clone(),
    }
}

/// Human-readable `ask` output, one block per question, in question order.
pub(crate) fn render_ask_text(questions: &Questions, response: &Response) -> Result<String, Failure> {
    let mut out = String::new();
    for (id, question) in questions {
        let Some(answer) = response.answers.get(id) else {
            return Err(Failure::Api(format!("the response has no answer for `{id}`")));
        };
        match (question, answer) {
            (Question::Noul(_), Answer::Noul(noul)) => {
                let _ = writeln!(out, "{id}: {:.2}", noul.noul);
            }
            (Question::Choice(_), Answer::Choice(choice)) => {
                let _ = writeln!(out, "{id}: {} (confidence {:.2})", choice.choice, choice.confidence);
                for (name, probability) in &choice.probabilities {
                    let _ = writeln!(out, "  {name}: {probability:.2}");
                }
            }
            (Question::Score(_), Answer::Score(score)) => {
                let _ = writeln!(out, "{id}: {:.2} (confidence {:.2})", score.score, score.confidence);
                for (level, probability) in &score.probabilities {
                    let description = score.legend.get(level).map(content_text).unwrap_or_default();
                    if description.is_empty() {
                        let _ = writeln!(out, "  {level}: {probability:.2}");
                    } else {
                        let _ = writeln!(out, "  {level} {description}: {probability:.2}");
                    }
                }
            }
            _ => {
                return Err(Failure::Api(format!(
                    "the answer for `{id}` is {}, not {}",
                    answer_kind(answer),
                    question_kind(question)
                )));
            }
        }
    }
    Ok(out)
}

/// The yes-probability, full precision, one line.
pub(crate) fn render_noul_text(response: &Response, id: &str) -> Result<String, Failure> {
    let answer = required_noul(response, id)?;
    Ok(format!("{}\n", answer.noul))
}

/// The chosen option name, one line.
pub(crate) fn render_choice_text(response: &Response, id: &str) -> Result<String, Failure> {
    let Some(answer) = response.choice(id) else {
        return Err(Failure::Api(mismatch(response, id, "a choice")));
    };
    Ok(format!("{}\n", answer.choice))
}

/// The score, full precision, one line.
pub(crate) fn render_score_text(response: &Response, id: &str) -> Result<String, Failure> {
    let Some(answer) = response.score(id) else {
        return Err(Failure::Api(mismatch(response, id, "a score")));
    };
    Ok(format!("{}\n", answer.score))
}

/// The noul answer, or an API error that names the id and not the payload.
pub(crate) fn required_noul<'a>(response: &'a Response, id: &str) -> Result<&'a typesafe_jev::NoulAnswer, Failure> {
    response.noul(id).ok_or_else(|| Failure::Api(mismatch(response, id, "a noul")))
}

/// One stderr line after a successful request. The cost is six digits. The word is always `retries`.
pub(crate) fn verbose_line(model: &str, input_tokens: u64, output_tokens: u64, retries: u64, cost_usd: f64) -> String {
    format!("{model}: {input_tokens} input tokens, {output_tokens} output tokens, {retries} retries, ~${cost_usd:.6}\n")
}

fn content_text(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        Content::Object(_) | Content::Array(_) => serde_json::to_string(content).unwrap_or_default(),
    }
}

fn mismatch(response: &Response, id: &str, expected: &str) -> String {
    match response.answers.get(id) {
        None => format!("the response has no answer for `{id}`"),
        Some(answer) => format!("the answer for `{id}` is {}, not {expected}", answer_kind(answer)),
    }
}

/// `a noul`, `a choice` or `a score`, for messages.
pub(crate) fn answer_kind(answer: &Answer) -> &'static str {
    match answer {
        Answer::Noul(_) => "a noul",
        Answer::Choice(_) => "a choice",
        Answer::Score(_) => "a score",
        // `Answer` is `non_exhaustive`.
        #[allow(clippy::wildcard_enum_match_arm)]
        _ => "an unknown answer",
    }
}

fn question_kind(question: &Question) -> &'static str {
    match question {
        Question::Noul(_) => "a noul",
        Question::Choice(_) => "a choice",
        Question::Score(_) => "a score",
        // `Question` is `non_exhaustive`.
        #[allow(clippy::wildcard_enum_match_arm)]
        _ => "an unknown question",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        render_ask_text, render_choice_text, render_noul_text, render_response, render_score_text, verbose_line,
    };
    use crate::questions::example_questions;
    use typesafe_jev::Response;

    const ASK_RESPONSE: &str = r#"{
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

    #[test]
    fn ask_text_matches_the_sample_and_question_order() {
        let response: Response = serde_json::from_str(ASK_RESPONSE).unwrap();
        let text = render_ask_text(&example_questions(), &response).unwrap();
        assert_eq!(text, ASK_TEXT);
    }

    #[test]
    fn a_structured_level_is_compact_json() {
        let response: Response = serde_json::from_str(
            r#"{
                "model": "m",
                "answers": {
                    "q": {
                        "type": "score",
                        "score": 0.5,
                        "confidence": 0.5,
                        "legend": {"0": {"label": "low"}, "1": "high"},
                        "probabilities": {"0": 0.4, "1": 0.6}
                    }
                }
            }"#,
        )
        .unwrap();
        let questions = crate::questions::one("q", typesafe_jev::Score::new("rate", ["low", "high"])).unwrap();
        let text = render_ask_text(&questions, &response).unwrap();
        assert_eq!(
            text,
            "\
q: 0.50 (confidence 0.50)
  0 {\"label\":\"low\"}: 0.40
  1 high: 0.60
"
        );
    }

    #[test]
    fn a_missing_level_description_has_no_gap_before_the_colon() {
        let response: Response = serde_json::from_str(
            r#"{
                "model": "m",
                "answers": {
                    "q": {
                        "type": "score",
                        "score": 1.0,
                        "confidence": 0.5,
                        "legend": {"0": "low", "1": ""},
                        "probabilities": {"0": 0.4, "1": 0.6, "2": 0.0}
                    }
                }
            }"#,
        )
        .unwrap();
        let questions = crate::questions::one("q", typesafe_jev::Score::new("rate", ["low", "mid", "high"])).unwrap();
        let text = render_ask_text(&questions, &response).unwrap();
        assert_eq!(
            text,
            "\
q: 1.00 (confidence 0.50)
  0 low: 0.40
  1: 0.60
  2: 0.00
"
        );
    }

    #[test]
    fn single_question_text_is_full_precision() {
        let response: Response = serde_json::from_str(ASK_RESPONSE).unwrap();
        assert_eq!(render_noul_text(&response, "is_urgent").unwrap(), "0.95\n");
        assert_eq!(render_choice_text(&response, "department").unwrap(), "technical\n");
        assert_eq!(render_score_text(&response, "frustration").unwrap(), "1\n");
        let precise: Response = serde_json::from_str(
            r#"{"model":"m","answers":{"q":{"type":"score","score":1.25,"confidence":0.5,"legend":{"0":"a","1":"b"},"probabilities":{"0":0.5,"1":0.5}}}}"#,
        )
        .unwrap();
        assert_eq!(render_score_text(&precise, "q").unwrap(), "1.25\n");
    }

    #[test]
    fn json_appends_cost_and_keeps_field_order() {
        let response: Response = serde_json::from_str(ASK_RESPONSE).unwrap();
        let cost = 318.0 / 1_000_000.0 * typesafe_jev::USD_PER_INPUT_MTOK;
        let json = render_response(&response, cost).unwrap();
        assert!(json.ends_with('\n'));
        let model = json.find("\"model\"").unwrap();
        let answers = json.find("\"answers\"").unwrap();
        let usage = json.find("\"usage\"").unwrap();
        let cost_at = json.find("\"cost_usd\"").unwrap();
        assert!(model < answers && answers < usage && usage < cost_at, "{json}");
        // Answer order is the API's order, which in this fixture is not alphabetical.
        let urgent = json.find("\"is_urgent\"").unwrap();
        let department = json.find("\"department\"").unwrap();
        assert!(urgent < department, "{json}");
    }

    #[test]
    fn verbose_line_matches_the_sample() {
        let cost = 318.0 / 1_000_000.0 * typesafe_jev::USD_PER_INPUT_MTOK;
        let line = verbose_line("jev-1.13.0", 318, 34, 0, cost);
        assert_eq!(line, "jev-1.13.0: 318 input tokens, 34 output tokens, 0 retries, ~$0.000013\n");
    }
}
