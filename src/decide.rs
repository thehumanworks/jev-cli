//! Apply a preset's rules to the answers of one request.
//!
//! This is pure: it does no I/O. Every rule is evaluated, even after the first match, so the output
//! can show when two rules agreed. A margin is how far a value is from flipping its condition, and
//! is positive when the condition holds. A score's margin is divided by `levels - 1`, so every
//! margin is between -1 and 1. For `is` and `in` the margin is the best probability inside the set
//! minus the best probability outside it.

use typesafe_jev::{Answer, ChoiceAnswer, Response};

use crate::exit::Failure;
use crate::preset::{Condition, Preset, Test, Then};
use crate::render::answer_kind;

/// The outcome of applying a preset to one response.
pub(crate) struct Decision<'a> {
    /// The chosen action. `None` only when no rule matched and there is no fallback.
    pub(crate) action: Option<String>,
    /// The 1-based number of the first rule that matched. `None` when no rule did.
    pub(crate) rule: Option<usize>,
    /// Every rule, in order.
    pub(crate) rules: Vec<RuleCheck<'a>>,
}

/// One rule, evaluated.
pub(crate) struct RuleCheck<'a> {
    /// Every condition held.
    pub(crate) matched: bool,
    /// Each condition, in order.
    pub(crate) conditions: Vec<ConditionCheck<'a>>,
}

/// One condition, evaluated.
pub(crate) struct ConditionCheck<'a> {
    /// The condition from the preset.
    pub(crate) condition: &'a Condition,
    /// What the answer said.
    pub(crate) value: Observed,
    /// The condition held.
    pub(crate) met: bool,
    /// Distance from flipping. Positive when the condition held.
    pub(crate) margin: f64,
}

/// The part of an answer a condition looked at.
pub(crate) enum Observed {
    /// A probability, a score or a confidence.
    Number(f64),
    /// The option a choice chose.
    Option(String),
}

/// Evaluate every rule against `response` and pick the action.
///
/// An answer that is missing, or of a different type than its question, is an API failure: the
/// crate already checked the reply's shape, so this means the API answered a different question.
pub(crate) fn decide<'a>(preset: &'a Preset, response: &Response) -> Result<Decision<'a>, Failure> {
    let mut rules = Vec::with_capacity(preset.rules.len());
    let mut first = None;
    for (index, rule) in preset.rules.iter().enumerate() {
        let conditions =
            rule.conditions.iter().map(|condition| check(condition, response)).collect::<Result<Vec<_>, _>>()?;
        let matched = conditions.iter().all(|checked| checked.met);
        if matched && first.is_none() {
            first = Some((index + 1, &rule.then));
        }
        rules.push(RuleCheck { matched, conditions });
    }
    let action = match first {
        Some((_, then)) => Some(chosen(preset, then, response)?),
        None => preset.fallback.clone(),
    };
    Ok(Decision { action, rule: first.map(|(number, _)| number), rules })
}

/// The condition that came closest to changing the decision, with its rule number.
///
/// The matched rule stops matching when any one condition flips, so its distance is its smallest
/// margin. A rule before it would match only if every unmet condition flipped, so its distance is
/// its largest unmet margin. When no rule matched, every rule counts as "before". Rules after the
/// match cannot change the decision.
pub(crate) fn closest_call<'d, 'a>(decision: &'d Decision<'a>) -> Option<(usize, &'d ConditionCheck<'a>)> {
    let last = decision.rule.unwrap_or(decision.rules.len());
    let mut best: Option<(f64, usize, &ConditionCheck<'a>)> = None;
    for (index, rule) in decision.rules.iter().take(last).enumerate() {
        let finite = rule.conditions.iter().filter(|checked| checked.margin.is_finite());
        let candidate = if rule.matched {
            finite.min_by(|a, b| a.margin.total_cmp(&b.margin)).map(|checked| (checked.margin, checked))
        } else {
            finite
                .filter(|checked| !checked.met)
                .min_by(|a, b| a.margin.total_cmp(&b.margin))
                .map(|checked| (-checked.margin, checked))
        };
        if let Some((distance, checked)) = candidate
            && best.is_none_or(|(closest, _, _)| distance < closest)
        {
            best = Some((distance, index + 1, checked));
        }
    }
    best.map(|(_, number, checked)| (number, checked))
}

fn chosen(preset: &Preset, then: &Then, response: &Response) -> Result<String, Failure> {
    let id = match then {
        Then::Action(name) => return Ok(name.clone()),
        Then::From(id) => id,
    };
    let Some(answer) = response.choice(id) else {
        return Err(Failure::Api(missing_or_mismatched(response, id, "a choice")));
    };
    if preset.action(&answer.choice).is_none() {
        return Err(Failure::Api(format!("the answer for `{id}` is `{}`, which is not an action", answer.choice)));
    }
    Ok(answer.choice.clone())
}

fn check<'a>(condition: &'a Condition, response: &Response) -> Result<ConditionCheck<'a>, Failure> {
    let id = condition.question.as_str();
    let Some(answer) = response.answers.get(id) else {
        return Err(Failure::Api(format!("the response has no answer for `{id}`")));
    };
    let width = condition.width;
    let (value, met, margin) = match (&condition.test, answer) {
        (Test::AtLeast(threshold), Answer::Noul(noul)) => at_least(noul.noul, *threshold, width),
        (Test::AtLeast(threshold), Answer::Score(score)) => at_least(score.score, *threshold, width),
        (Test::Below(threshold), Answer::Noul(noul)) => below(noul.noul, *threshold, width),
        (Test::Below(threshold), Answer::Score(score)) => below(score.score, *threshold, width),
        (Test::ConfidenceAtLeast(threshold), Answer::Choice(choice)) => at_least(choice.confidence, *threshold, 1.0),
        (Test::ConfidenceAtLeast(threshold), Answer::Score(score)) => at_least(score.confidence, *threshold, 1.0),
        (Test::Is(option), Answer::Choice(choice)) => options(choice, |name| name == option),
        (Test::In(set), Answer::Choice(choice)) => options(choice, |name| set.iter().any(|option| option == name)),
        (test, _) => {
            let expected = match test {
                Test::AtLeast(_) | Test::Below(_) => "a noul or a score",
                Test::Is(_) | Test::In(_) => "a choice",
                Test::ConfidenceAtLeast(_) => "a choice or a score",
            };
            return Err(Failure::Api(format!("the answer for `{id}` is {}, not {expected}", answer_kind(answer))));
        }
    };
    Ok(ConditionCheck { condition, value, met, margin })
}

/// A non-finite value never meets a threshold: every comparison with NaN is false.
fn at_least(value: f64, threshold: f64, width: f64) -> (Observed, bool, f64) {
    (Observed::Number(value), value >= threshold, (value - threshold) / width)
}

fn below(value: f64, threshold: f64, width: f64) -> (Observed, bool, f64) {
    (Observed::Number(value), value < threshold, (threshold - value) / width)
}

fn options(answer: &ChoiceAnswer, inside: impl Fn(&str) -> bool) -> (Observed, bool, f64) {
    let best = |wanted: bool| {
        answer
            .probabilities
            .iter()
            .filter(|(name, _)| inside(name) == wanted)
            .map(|(_, probability)| *probability)
            .fold(0.0, f64::max)
    };
    (Observed::Option(answer.choice.clone()), inside(&answer.choice), best(true) - best(false))
}

fn missing_or_mismatched(response: &Response, id: &str, expected: &str) -> String {
    match response.answers.get(id) {
        None => format!("the response has no answer for `{id}`"),
        Some(answer) => format!("the answer for `{id}` is {}, not {expected}", answer_kind(answer)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::{Value, json};
    use typesafe_jev::Response;

    use super::{Observed, closest_call, decide};
    use crate::preset::{Preset, parse_preset};

    const CI: &str = include_str!("../tests/fixtures/presets/ci.json");

    /// A flaky failure: rule 3 picks `retry`.
    fn flaky() -> Value {
        json!({
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
        })
    }

    fn with(mut response: Value, pointer: &str, value: Value) -> Value {
        *response.pointer_mut(pointer).unwrap() = value;
        response
    }

    fn response(value: Value) -> Response {
        serde_json::from_value(value).unwrap()
    }

    fn preset() -> Preset {
        parse_preset(CI).unwrap()
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn the_first_matching_rule_decides() {
        let preset = preset();
        let decision = decide(&preset, &response(flaky())).unwrap();
        assert_eq!(decision.action.as_deref(), Some("retry"));
        assert_eq!(decision.rule, Some(3));
        assert_eq!(decision.rules.iter().map(|rule| rule.matched).collect::<Vec<_>>(), [false, false, true]);

        let secret = response(with(flaky(), "/answers/secrets/noul", json!(0.41)));
        let decision = decide(&preset, &secret).unwrap();
        assert_eq!((decision.action.as_deref(), decision.rule), (Some("escalate"), Some(1)));
        assert!(decision.rules[2].matched, "a later rule that also matched is still reported");

        let broad = with(flaky(), "/answers/kind/choice", json!("issue"));
        let broad = with(broad, "/answers/scope/score", json!(1.7));
        let decision = decide(&preset, &response(broad)).unwrap();
        assert_eq!((decision.action.as_deref(), decision.rule), (Some("escalate"), Some(2)));

        let unsure = response(with(flaky(), "/answers/kind/confidence", json!(0.41)));
        let decision = decide(&preset, &unsure).unwrap();
        assert_eq!((decision.action.as_deref(), decision.rule), (Some("escalate"), None));
    }

    #[test]
    fn no_match_and_no_fallback_is_no_decision() {
        let mut text: Value = serde_json::from_str(CI).unwrap();
        text.as_object_mut().unwrap().remove("fallback");
        let preset = parse_preset(&text.to_string()).unwrap();
        let unsure = response(with(flaky(), "/answers/kind/confidence", json!(0.41)));
        let decision = decide(&preset, &unsure).unwrap();
        assert_eq!((decision.action, decision.rule), (None, None));
    }

    #[test]
    fn margins_are_signed_and_scores_are_scaled() {
        let preset = preset();
        let decision = decide(&preset, &response(flaky())).unwrap();
        let margins: Vec<Vec<f64>> =
            decision.rules.iter().map(|rule| rule.conditions.iter().map(|checked| checked.margin).collect()).collect();
        close(margins[0][0], 0.03 - 0.3);
        close(margins[1][0], 0.08 - 0.90);
        close(margins[1][1], (0.3 - 1.5) / 2.0);
        close(margins[2][0], 0.84 - 0.6);
        assert!(matches!(&decision.rules[1].conditions[0].value, Observed::Option(option) if option == "retry"));
        assert!(matches!(decision.rules[0].conditions[0].value, Observed::Number(p) if (p - 0.03).abs() < 1e-9));

        let (rule, checked) = closest_call(&decision).unwrap();
        assert_eq!((rule, checked.condition.question.as_str()), (3, "kind"));
        close(checked.margin, 0.24);
    }

    #[test]
    fn in_measures_the_best_option_inside_against_the_best_outside() {
        let text = CI.replace(r#"{ "is": "issue" }"#, r#"{ "in": ["issue", "fmt"] }"#);
        let preset = parse_preset(&text).unwrap();
        let decision = decide(&preset, &response(flaky())).unwrap();
        let checked = &decision.rules[1].conditions[0];
        assert!(!checked.met);
        close(checked.margin, 0.08 - 0.90);

        let fmt = with(flaky(), "/answers/kind/choice", json!("fmt"));
        let fmt = with(fmt, "/answers/kind/probabilities", json!({"retry": 0.3, "fmt": 0.6, "issue": 0.1}));
        let decision = decide(&preset, &response(fmt)).unwrap();
        let checked = &decision.rules[1].conditions[0];
        assert!(checked.met);
        close(checked.margin, 0.6 - 0.3);
    }

    #[test]
    fn the_closest_call_ignores_rules_after_the_match() {
        let preset = preset();
        // Rule 1 matches narrowly (0.31 against 0.3). Rule 3 matches too, by more, but comes later.
        let secret = response(with(flaky(), "/answers/secrets/noul", json!(0.31)));
        let decision = decide(&preset, &secret).unwrap();
        let (rule, checked) = closest_call(&decision).unwrap();
        assert_eq!(rule, 1);
        close(checked.margin, 0.01);

        // Nothing matched: the closest rule is the one whose hardest unmet condition is nearest.
        let unsure = response(with(flaky(), "/answers/kind/confidence", json!(0.55)));
        let decision = decide(&preset, &unsure).unwrap();
        let (rule, checked) = closest_call(&decision).unwrap();
        assert_eq!(rule, 3);
        close(checked.margin, -0.05);
    }

    #[test]
    fn a_non_finite_probability_meets_nothing() {
        let preset = preset();
        let mut nan = response(flaky());
        if let Some(typesafe_jev::Answer::Noul(noul)) = nan.answers.get_mut("secrets") {
            noul.noul = f64::NAN;
        }
        let decision = decide(&preset, &nan).unwrap();
        assert!(!decision.rules[0].conditions[0].met);
        assert_eq!(decision.action.as_deref(), Some("retry"));
        let (rule, _) = closest_call(&decision).unwrap();
        assert_eq!(rule, 3, "a NaN margin is never the closest call");
    }

    #[test]
    fn a_missing_or_mistyped_answer_is_an_api_failure() {
        let preset = preset();
        let mut missing = flaky();
        missing["answers"].as_object_mut().unwrap().remove("scope");
        let failure = decide(&preset, &response(missing)).err().unwrap();
        assert_eq!((failure.code(), failure.message()), (6, "the response has no answer for `scope`"));

        let mistyped = with(
            flaky(),
            "/answers/secrets",
            json!({"type": "choice", "choice": "a", "confidence": 1.0, "probabilities": {"a": 1.0}}),
        );
        let failure = decide(&preset, &response(mistyped)).err().unwrap();
        assert_eq!(failure.message(), "the answer for `secrets` is a choice, not a noul or a score");

        let unknown = with(flaky(), "/answers/kind/choice", json!("reboot"));
        let failure = decide(&preset, &response(unknown)).err().unwrap();
        assert_eq!(failure.message(), "the answer for `kind` is `reboot`, which is not an action");
    }
}
