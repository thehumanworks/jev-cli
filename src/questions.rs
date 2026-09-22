//! Build a [`typesafe_jev::Questions`] value from a subcommand or from a JSON document.
//!
//! The document format is the crate's own serde format. This module does not invent a second
//! schema: it deserializes [`Questions`] and, for the single-question commands, fills the same
//! types the document would have contained. A repeated id is rejected; serde's usual last-wins
//! rule would hide the earlier question.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use typesafe_jev::{Choice, Noul, Question, Questions, Score};

use crate::exit::Failure;
use crate::state::strip_bom;

/// Shown when `ask` has neither `-q` nor `--questions-json`.
pub(crate) const MISSING_QUESTIONS: &str = "pass questions with -q PATH or --questions-json JSON";

/// Shown when the document is empty or only whitespace.
pub(crate) const EMPTY_QUESTIONS: &str = "the questions document is empty";

/// Deserialize a questions document. The error includes serde's description.
pub(crate) fn questions_from_json(text: &str) -> Result<Questions, Failure> {
    let text = strip_bom(text);
    if text.trim().is_empty() {
        return Err(Failure::Usage(EMPTY_QUESTIONS.into()));
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let questions = deserializer.deserialize_map(QuestionMap).map_err(|err| questions_error(&err))?;
    deserializer.end().map_err(|err| questions_error(&err))?;
    if questions.is_empty() {
        return Err(Failure::Usage(EMPTY_QUESTIONS.into()));
    }
    Ok(questions)
}

struct QuestionMap;

impl<'de> Visitor<'de> for QuestionMap {
    type Value = Questions;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a JSON object of questions")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut questions = Questions::new();
        let mut seen = HashSet::new();
        while let Some(id) = map.next_key::<String>()? {
            if !seen.insert(id.clone()) {
                return Err(de::Error::custom(format!("the questions document has the id \"{id}\" twice")));
            }
            let question = map.next_value::<Question>()?;
            questions.insert(id, question);
        }
        Ok(questions)
    }
}

fn questions_error(err: &serde_json::Error) -> Failure {
    const MARKER: &str = "the questions document has the id ";
    let msg = err.to_string();
    if let Some(start) = msg.find(MARKER) {
        let tail = msg.get(start..).unwrap_or(msg.as_str());
        let end = tail.find(" at line ").unwrap_or(tail.len());
        let phrase = tail.get(..end).unwrap_or(tail);
        return Failure::Usage(phrase.to_owned());
    }
    Failure::Usage(format!("the questions document is not valid: {msg}"))
}

/// One question under `id`. An empty id cannot be selected later.
pub(crate) fn one(id: &str, question: impl Into<Question>) -> Result<Questions, Failure> {
    if id.is_empty() {
        return Err(Failure::Usage("question id is empty".into()));
    }
    Ok(Questions::new().with(id, question))
}

/// A yes/no question. `--yes` and `--no` become the `true` and `false` criteria.
pub(crate) fn noul_question(instructions: &str, yes: Option<&str>, no: Option<&str>) -> Noul {
    let mut noul = Noul::new(instructions);
    if let Some(yes) = yes {
        noul = noul.yes(yes);
    }
    if let Some(no) = no {
        noul = noul.no(no);
    }
    noul
}

/// `P` for `--threshold`: finite and inside `0..=1`.
pub(crate) fn check_threshold(probability: f64) -> Result<(), Failure> {
    if probability.is_finite() && (0.0..=1.0).contains(&probability) {
        Ok(())
    } else {
        Err(Failure::Usage("threshold must be between 0 and 1".into()))
    }
}

/// Parse `name` and `name=description` options. Names are split on the first `=`.
pub(crate) fn parse_options(raw: &[String]) -> Result<Vec<(String, Option<String>)>, Failure> {
    let mut names = HashSet::with_capacity(raw.len());
    let mut options = Vec::with_capacity(raw.len());
    for item in raw {
        let (name, description) = parse_option(item)?;
        if !names.insert(name.clone()) {
            return Err(Failure::Usage(format!("duplicate choice option `{name}`")));
        }
        options.push((name, description));
    }
    let count = options.len();
    if !(2..=255).contains(&count) {
        return Err(Failure::Usage(format!("a choice needs between 2 and 255 options, not {count}")));
    }
    Ok(options)
}

/// Build a [`Choice`] from options that [`parse_options`] already accepted.
pub(crate) fn into_choice(instructions: &str, options: &[(String, Option<String>)]) -> Choice {
    let Some((first, rest)) = options.split_first() else {
        // `parse_options` rejects an empty list. This arm keeps the function total.
        return Choice::labels(instructions, std::iter::empty::<String>());
    };
    let mut choice = match &first.1 {
        Some(description) => Choice::new(instructions, [(first.0.as_str(), description.as_str())]),
        None => Choice::labels(instructions, [first.0.as_str()]),
    };
    for (name, description) in rest {
        choice = match description {
            Some(description) => choice.option(name, description),
            None => choice.label(name),
        };
    }
    choice
}

/// A rating. Levels are ordered from low to high, and there must be between 2 and 10 of them.
pub(crate) fn score_question(instructions: &str, levels: &[String]) -> Result<Score, Failure> {
    if levels.iter().any(|level| level.trim().is_empty()) {
        return Err(Failure::Usage("a score level is empty".into()));
    }
    let count = levels.len();
    if !(2..=10).contains(&count) {
        return Err(Failure::Usage(format!("a score needs between 2 and 10 levels, not {count}")));
    }
    Ok(Score::new(instructions, levels.iter().map(String::as_str)))
}

/// The support-ticket questions from the `typesafe-jev` README, one of each type.
pub(crate) fn example_questions() -> Questions {
    Questions::new()
        .with(
            "department",
            Choice::new(
                "Which team should handle this?",
                [
                    ("billing", "Payment or subscription issues"),
                    ("technical", "Bugs or integration problems"),
                    ("sales", "Pricing or account questions"),
                ],
            ),
        )
        .with(
            "frustration",
            Score::new(
                "How frustrated the customer appears",
                ["Calm, just stating facts", "Frustrated but civil", "Very angry, strong language"],
            ),
        )
        .with("is_urgent", Noul::new("The message conveys urgency or time-sensitivity"))
}

fn parse_option(raw: &str) -> Result<(String, Option<String>), Failure> {
    let (name, description) = match raw.split_once('=') {
        Some((name, description)) => (name, Some(description)),
        None => (raw, None),
    };
    if name.is_empty() {
        return Err(Failure::Usage("a choice option name is empty".into()));
    }
    if description.is_some_and(str::is_empty) {
        return Err(Failure::Usage(format!(
            "choice option `{name}` has an empty description; omit '=' when the name is enough"
        )));
    }
    Ok((name.to_owned(), description.map(str::to_owned)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{check_threshold, example_questions, into_choice, parse_options, questions_from_json, score_question};
    use typesafe_jev::{Question, Questions};

    #[test]
    fn options_split_on_the_first_equals_and_keep_order() {
        let options =
            parse_options(&["billing".into(), "technical=Bugs=and=outages".into(), "sales=Pricing".into()]).unwrap();
        assert_eq!(
            options,
            vec![
                ("billing".into(), None),
                ("technical".into(), Some("Bugs=and=outages".into())),
                ("sales".into(), Some("Pricing".into())),
            ]
        );
        let choice = into_choice("Which?", &options);
        assert_eq!(choice.criteria.keys().map(String::as_str).collect::<Vec<_>>(), ["billing", "technical", "sales"]);
        assert_eq!(choice.criteria["billing"], None);
        assert_eq!(
            choice.criteria["technical"].as_ref().and_then(|content| content.as_str()),
            Some("Bugs=and=outages")
        );
    }

    #[test]
    fn option_names_must_be_present_unique_and_within_range() {
        assert!(parse_options(&["=desc".into(), "other".into()]).unwrap_err().message().contains("name is empty"));
        assert!(parse_options(&["name=".into(), "other".into()]).unwrap_err().message().contains("empty description"));
        assert!(parse_options(&["a".into(), "a=desc".into()]).unwrap_err().message().contains("duplicate"));
        assert!(parse_options(&["only".into()]).unwrap_err().message().contains("between 2 and 255"));
        let mut many: Vec<String> = (0..256).map(|n| format!("o{n}")).collect();
        assert!(parse_options(&many).unwrap_err().message().contains("not 256"));
        many.pop();
        assert_eq!(parse_options(&many).unwrap().len(), 255);
    }

    #[test]
    fn score_level_count_is_enforced() {
        assert!(score_question("rate", &["only".into()]).is_err());
        assert_eq!(
            score_question("rate", &[String::new(), "x".into()]).unwrap_err().message(),
            "a score level is empty"
        );
        assert_eq!(
            score_question("rate", &["low".into(), "  \t".into()]).unwrap_err().message(),
            "a score level is empty"
        );
        assert!(score_question("rate", &["low".into(), "high".into()]).is_ok());
        let ten: Vec<String> = (0..10).map(|n| format!("L{n}")).collect();
        assert_eq!(score_question("rate", &ten).unwrap().criteria.len(), 10);
        let eleven: Vec<String> = (0..11).map(|n| format!("L{n}")).collect();
        assert!(score_question("rate", &eleven).unwrap_err().message().contains("not 11"));
    }

    #[test]
    fn threshold_bounds() {
        for raw in ["0", "1", "0.5", "0.0"] {
            let value: f64 = raw.parse().unwrap();
            assert!(check_threshold(value).is_ok(), "{raw}");
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(check_threshold(value).is_err());
        }
        let too_high: f64 = "1.0001".parse().unwrap();
        let negative: f64 = "-0.1".parse().unwrap();
        assert!(check_threshold(too_high).is_err());
        assert!(check_threshold(negative).is_err());
    }

    #[test]
    fn documents_round_trip_and_bad_ones_quote_serde() {
        assert!(questions_from_json("").unwrap_err().message().contains("empty"));
        assert!(questions_from_json("{}").unwrap_err().message().contains("empty"));
        let err = questions_from_json(r#"{"q":{"instructions":"x"}}"#).unwrap_err();
        assert!(err.message().contains("missing field"), "{}", err.message());
        assert!(err.message().contains("type"), "{}", err.message());
        let err = questions_from_json(r#"{"q":{"type":"ranking"}}"#).unwrap_err();
        assert!(err.message().contains("ranking"), "{}", err.message());

        let questions = example_questions();
        let json = serde_json::to_string(&questions).unwrap();
        let back: Questions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, questions);
        assert!(matches!(back.get("is_urgent"), Some(Question::Noul(_))));
    }

    #[test]
    fn a_bom_is_ignored_and_a_repeated_id_is_rejected() {
        let bom = "\u{feff}{\"q\":{\"type\":\"noul\",\"instructions\":\"x\"}}";
        assert!(questions_from_json(bom).is_ok());
        let dup = r#"{"q":{"type":"noul","instructions":"a"},"q":{"type":"noul","instructions":"b"}}"#;
        let err = questions_from_json(dup).unwrap_err();
        assert_eq!(err.message(), "the questions document has the id \"q\" twice");
    }
}
