//! Failures the CLI can report, and the exit codes they map to.
//!
//! A response body is not printed unless the caller asked for `--debug`. The crate's own messages
//! never quote the request or the API key; when they do quote a response body, it follows an
//! `HTTP <status>` marker (or, for a context-length error, the message is the body itself).

use std::fmt;

use typesafe_jev::Error;

/// Shown in place of a response body.
const DEBUG_HINT: &str = "; re-run with --debug for the response body";

/// A failure that already knows which process status it should produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The invocation itself is wrong.
    Usage(String),
    /// No API key, or the API rejected it.
    Auth(String),
    /// The API refused the request, or the client configuration cannot be sent.
    Rejected(String),
    /// The request does not fit in the model's context.
    TokenLimit(String),
    /// Any other API or network failure, including retries exhausted.
    Api(String),
    /// A local file or stream could not be read or written.
    Io(String),
    /// Stdout was closed. This is success: the consumer has what it asked for.
    BrokenPipe,
}

impl Failure {
    /// Map a client error onto the CLI's exit codes.
    ///
    /// `debug` keeps the crate's message, including any response body. Otherwise the body is
    /// dropped.
    pub(crate) fn from_api(error: &Error, debug: bool) -> Self {
        // `Error` is `#[non_exhaustive]`: the wildcard arms cover variants added later.
        let visible = match error {
            // The crate stores the response body itself here, with no `HTTP <status>` prefix.
            Error::TokenLimit(_) if !debug => format!("the request exceeds the model's context{DEBUG_HINT}"),
            _ => visible_api_message(error.message(), debug),
        };
        match error {
            Error::Auth(_) => Self::Auth(visible),
            Error::InvalidRequest(_) | Error::InvalidConfig(_) => Self::Rejected(visible),
            Error::TokenLimit(_) => Self::TokenLimit(visible),
            _ => Self::Api(visible),
        }
    }

    /// The process status for this failure.
    #[must_use]
    pub(crate) fn code(&self) -> u8 {
        match self {
            Self::BrokenPipe => 0,
            Self::Usage(_) => 2,
            Self::Auth(_) => 3,
            Self::Rejected(_) => 4,
            Self::TokenLimit(_) => 5,
            Self::Api(_) => 6,
            Self::Io(_) => 7,
        }
    }

    /// The message, without the `jev:` prefix and without a trailing newline.
    #[must_use]
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Usage(message)
            | Self::Auth(message)
            | Self::Rejected(message)
            | Self::TokenLimit(message)
            | Self::Api(message)
            | Self::Io(message) => message,
            Self::BrokenPipe => "",
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Failure {}

/// The text to show for a crate error.
///
/// With `debug`, the message is kept (collapsed to one line). Otherwise a response body that
/// follows an `HTTP <status>` marker is dropped, and the hint to re-run with `--debug` is appended.
/// Messages with no such marker are returned unchanged: connection failures and local serde errors
/// do not carry a response body.
#[must_use]
pub(crate) fn visible_api_message(message: &str, debug: bool) -> String {
    let flat = one_line(message);
    if debug {
        return flat;
    }
    let Some(end) = http_marker_end(&flat) else {
        return flat;
    };
    let Some(head) = flat.get(..end) else {
        return flat;
    };
    format!("{head}{DEBUG_HINT}")
}

/// Byte index just past `HTTP <digits>` and a closing `)` when the status is parenthesized.
fn http_marker_end(message: &str) -> Option<usize> {
    let mut rest = message;
    let mut offset = 0;
    while let Some(rel) = rest.find("HTTP ") {
        let after = offset + rel + "HTTP ".len();
        let tail = message.get(after..)?;
        let digits = tail.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            // ASCII digits are one byte each, so the char count is the byte length.
            let mut end = after + digits;
            if message.get(end..).is_some_and(|text| text.starts_with(')')) {
                end += 1;
            }
            return Some(end);
        }
        let step = rel + "HTTP ".len();
        rest = rest.get(step..)?;
        offset += step;
    }
    None
}

/// One physical line. Empty input becomes a placeholder so stderr is never `jev:` alone.
pub(crate) fn one_line(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut pending_space = false;
    for ch in message.chars() {
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    if out.is_empty() { "unknown error".to_owned() } else { out }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Failure, one_line, visible_api_message};
    use typesafe_jev::Error;

    #[test]
    fn each_api_variant_has_its_exit_code() {
        let cases = [
            (Error::Auth("no".into()), 3),
            (Error::InvalidRequest("bad".into()), 4),
            (Error::InvalidConfig("bad".into()), 4),
            (Error::TokenLimit("big".into()), 5),
            (Error::Api("nope".into()), 6),
        ];
        for (error, code) in cases {
            assert_eq!(Failure::from_api(&error, false).code(), code);
        }
        assert_eq!(Failure::Usage("x".into()).code(), 2);
        assert_eq!(Failure::Io("x".into()).code(), 7);
        assert_eq!(Failure::BrokenPipe.code(), 0);
    }

    #[test]
    fn response_bodies_are_dropped_unless_debug_is_set() {
        let body = "S".repeat(500);
        let cases = [
            format!("HTTP 422: {body}"),
            format!("gave up after 8 retries: HTTP 500: {body}"),
            format!("TypeSafe rejected the API key (HTTP 401): {body}"),
        ];
        for message in &cases {
            let hidden = visible_api_message(message, false);
            assert!(!hidden.contains(&body), "{hidden}");
            assert!(hidden.contains("HTTP "), "{hidden}");
            assert!(hidden.ends_with("; re-run with --debug for the response body"), "{hidden}");
            let shown = visible_api_message(message, true);
            assert!(shown.contains(&body), "{shown}");
        }
        assert_eq!(
            visible_api_message("HTTP 422: {\"error\":\"nope\"}", false),
            "HTTP 422; re-run with --debug for the response body"
        );
        assert_eq!(
            visible_api_message("gave up after 2 retries: HTTP 503: down", false),
            "gave up after 2 retries: HTTP 503; re-run with --debug for the response body"
        );
        assert_eq!(
            visible_api_message("TypeSafe rejected the API key (HTTP 401): nope", false),
            "TypeSafe rejected the API key (HTTP 401); re-run with --debug for the response body"
        );
    }

    #[test]
    fn messages_without_an_http_status_are_kept() {
        for message in [
            "gave up after 0 retries: connection reset",
            "the state does not serialize to JSON: eof",
            "unreadable response: expected ident at line 1 column 1",
        ] {
            assert_eq!(visible_api_message(message, false), message);
        }
    }

    #[test]
    fn a_token_limit_body_is_not_forwarded() {
        let body = format!("max_tokens_exceeded {}", "S".repeat(500));
        let hidden = Failure::from_api(&Error::TokenLimit(body.clone()), false);
        assert!(!hidden.message().contains('S'), "{}", hidden.message());
        assert!(hidden.message().contains("re-run with --debug"));
        let shown = Failure::from_api(&Error::TokenLimit(body.clone()), true);
        assert!(shown.message().contains(&body));
    }

    #[test]
    fn one_line_collapses_blank_messages() {
        assert_eq!(one_line(" \n\t"), "unknown error");
        assert_eq!(one_line("two\nlines"), "two lines");
    }
}
