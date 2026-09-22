//! Where the state comes from, and the value that is sent as JSON.
//!
//! Text is sent as a JSON string. `--state-json` accepts only an object, an array or a string,
//! which is the same set the API calls content. Object key order is preserved so a dry run shows
//! the document the user wrote.

use std::io::Read;
use std::path::Path;

use serde::Serializer;
use serde_json::Value;

use crate::exit::Failure;

/// The state of one request, already checked.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum State {
    /// Plain text. Serialized as a JSON string.
    Text(String),
    /// A JSON object, array or string.
    Json(Value),
}

impl serde::Serialize for State {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text(text) => serializer.serialize_str(text),
            Self::Json(value) => value.serialize(serializer),
        }
    }
}

/// Where to read a blob of text from.
#[derive(Clone, Copy)]
pub(crate) enum Origin<'a> {
    /// The value was already on the command line.
    Text(&'a str),
    /// A filesystem path.
    File(&'a Path),
    /// Standard input.
    Stdin,
}

/// Read `origin`. `what` is `state` or `questions`, for the I/O error.
pub(crate) fn read_origin(origin: &Origin<'_>, what: &str, stdin: &mut dyn Read) -> Result<String, Failure> {
    match origin {
        Origin::Text(text) => Ok((*text).to_owned()),
        Origin::File(path) => std::fs::read_to_string(path)
            .map_err(|err| Failure::Io(format!("cannot read {what} from {}: {err}", path.display()))),
        Origin::Stdin => {
            let mut buf = String::new();
            stdin
                .read_to_string(&mut buf)
                .map_err(|err| Failure::Io(format!("cannot read {what} from stdin: {err}")))?;
            Ok(buf)
        }
    }
}

/// Drop one leading UTF-8 BOM so a file saved by an editor is still the document the user wrote.
pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Turn raw text into a [`State`]. Blank text is rejected. JSON numbers, booleans and null are rejected.
pub(crate) fn interpret_state(raw: &str, as_json: bool) -> Result<State, Failure> {
    let raw = strip_bom(raw);
    if !as_json {
        return if raw.trim().is_empty() {
            Err(Failure::Usage(EMPTY_STATE.into()))
        } else {
            Ok(State::Text(raw.to_owned()))
        };
    }
    let value =
        serde_json::from_str::<Value>(raw).map_err(|err| Failure::Usage(format!("state is not valid JSON: {err}")))?;
    match value {
        Value::Object(object) => Ok(State::Json(Value::Object(object))),
        Value::Array(items) => Ok(State::Json(Value::Array(items))),
        Value::String(text) if text.trim().is_empty() => Err(Failure::Usage(EMPTY_STATE.into())),
        Value::String(text) => Ok(State::Json(Value::String(text))),
        Value::Number(_) => {
            Err(Failure::Usage("state JSON must be an object, an array or a string, not a number".into()))
        }
        Value::Bool(_) => {
            Err(Failure::Usage("state JSON must be an object, an array or a string, not a boolean".into()))
        }
        Value::Null => Err(Failure::Usage("state JSON must be an object, an array or a string, not null".into())),
    }
}

/// Shown when stdin is a terminal and no state flag was passed.
pub(crate) const TERMINAL_STATE: &str =
    "no state was given and stdin is a terminal; pass -s TEXT, -f PATH, or pipe the state on stdin";

/// Shown when the text state is empty or whitespace.
pub(crate) const EMPTY_STATE: &str = "the state is empty";

/// The support-ticket state from the `typesafe-jev` README, for `jev example --state`.
pub(crate) const EXAMPLE_STATE: &str = "Hi, I've been trying to connect my Stripe account for 3 days and the \
     integration keeps failing. I'm losing sales. Please help ASAP.";

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{EMPTY_STATE, State, interpret_state};
    use crate::exit::Failure;

    fn message(result: Result<State, Failure>) -> String {
        result.unwrap_err().message().to_owned()
    }

    #[test]
    fn blank_text_is_rejected_and_surrounding_spaces_are_kept() {
        assert_eq!(message(interpret_state("  \n", false)), EMPTY_STATE);
        let state = interpret_state("  hi  ", false).unwrap();
        assert_eq!(serde_json::to_string(&state).unwrap(), r#""  hi  ""#);
    }

    #[test]
    fn json_state_accepts_object_array_and_string_only() {
        assert!(matches!(interpret_state("{}", true).unwrap(), State::Json(_)));
        assert!(matches!(interpret_state("[1]", true).unwrap(), State::Json(_)));
        assert!(matches!(interpret_state(r#""hi""#, true).unwrap(), State::Json(_)));
        assert_eq!(message(interpret_state(r#""   ""#, true)), EMPTY_STATE);
        assert!(message(interpret_state("1", true)).contains("number"));
        assert!(message(interpret_state("true", true)).contains("boolean"));
        assert!(message(interpret_state("null", true)).contains("null"));
    }

    #[test]
    fn json_object_key_order_is_preserved() {
        let state = interpret_state(r#"{"z":1,"a":2}"#, true).unwrap();
        let json = serde_json::to_string(&state).unwrap();
        let zed = json.find("\"z\"").unwrap();
        let ay = json.find("\"a\"").unwrap();
        assert!(zed < ay, "{json}");
    }

    #[test]
    fn a_leading_bom_is_not_part_of_the_state() {
        let text = interpret_state("\u{feff}hello", false).unwrap();
        assert_eq!(serde_json::to_string(&text).unwrap(), r#""hello""#);
        let value = interpret_state("\u{feff}{\"z\":1}", true).unwrap();
        assert!(matches!(value, State::Json(_)));
        assert_eq!(message(interpret_state("\u{feff}   ", false)), EMPTY_STATE);
    }

    #[test]
    fn invalid_json_does_not_quote_the_state() {
        let raw = "UNIQUE_STATE_SENTINEL_1234567890 !!!";
        let err = message(interpret_state(raw, true));
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(!err.contains("UNIQUE_STATE_SENTINEL"), "{err}");
    }
}
