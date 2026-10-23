//! The name a send writes a Message's file under, for every technology of
//! the file family — a folder, an SFTP server — written once (ADR-0024,
//! amendments 2026-10-20 and 2026-10-21): the name the Message was
//! received under, which its context keeps (`context::property::FILE_NAME`);
//! where it carries none, the Location's pattern, [`MESSAGE`] in it the
//! Message's identifier, or the identifier itself where it has no pattern;
//! or the name a test or a tool gave, where it sent bytes it holds.

use context::property::FILE_NAME;

use crate::error::{Result, TransportError};
use crate::outgoing::Outgoing;

/// What a Location's name pattern writes the Message's identifier as.
pub const MESSAGE: &str = "{message}";

/// The one file `outgoing` is written as in `place`, its directory, as the
/// module says: `pattern` the Location's, `given` the name a sender of held
/// bytes named.
///
/// # Errors
///
/// A Message carrying neither a file name nor an identifier, or a name
/// that is not one file's — empty, a path, `.` or `..` — permanently:
/// asking again names it no differently.
pub fn file_name(
    outgoing: &Outgoing<'_>,
    pattern: Option<&str>,
    given: &str,
    place: &str,
) -> Result<String> {
    let name = match outgoing.context() {
        Some(context) => context
            .get(FILE_NAME)
            .and_then(xcore::ScalarValue::text)
            .map(std::borrow::Cow::into_owned)
            .or_else(|| {
                outgoing
                    .message()
                    .map(|message| patterned(pattern, message))
            })
            .ok_or_else(|| {
                TransportError::permanent(format!(
                    "the Message carries no {FILE_NAME} and no identifier to name its file \
                     by in '{place}'"
                ))
            })?,
        None => given.to_string(),
    };
    let plain = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && std::path::Path::new(&name)
            .file_name()
            .is_some_and(|file| file == name.as_str());
    if !plain {
        return Err(TransportError::permanent(format!(
            "'{name}' names no file of its own in '{place}'"
        )));
    }
    Ok(name)
}

/// The name `pattern` gives the Message `message`, or the identifier itself
/// where there is none.
fn patterned(pattern: Option<&str>, message: &str) -> String {
    pattern.map_or_else(
        || message.to_string(),
        |pattern| pattern.replace(MESSAGE, message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received_as(name: &str) -> context::MessageContext {
        context::MessageContext::new()
            .with_value(FILE_NAME, xcore::ScalarValue::Text(name.to_string()))
    }

    #[test]
    fn the_received_name_goes_first_then_the_pattern_then_the_identifier() {
        let message = "0199f0a1-7c2e-7000-8000-000000000001";
        let named = received_as("order-7.edi");
        let held = Outgoing::held(b"x").carrying(&named).of_message(message);
        assert_eq!(
            file_name(&held, Some("o-{message}"), "", "out").expect("named"),
            "order-7.edi"
        );
        let unnamed = context::MessageContext::new();
        let held = Outgoing::held(b"x").carrying(&unnamed).of_message(message);
        assert_eq!(
            file_name(&held, Some("o-{message}"), "", "out").expect("named"),
            format!("o-{message}")
        );
        assert_eq!(file_name(&held, None, "", "out").expect("named"), message);
        let tool = Outgoing::held(b"x");
        assert_eq!(
            file_name(&tool, None, "probe.bin", "out").expect("named"),
            "probe.bin"
        );
    }

    #[test]
    fn no_name_or_a_path_for_one_is_refused_in_words_for_good() {
        let unnamed = context::MessageContext::new();
        let held = Outgoing::held(b"x").carrying(&unnamed);
        let refused = file_name(&held, None, "", "out").expect_err("nothing names it");
        assert!(refused.message.contains(FILE_NAME), "{}", refused.message);
        assert!(!refused.retryable);
        for name in ["", ".", "..", "a/b", "a\\b", "C:x"] {
            let path = received_as(name);
            let held = Outgoing::held(b"x").carrying(&path);
            let refused = file_name(&held, None, "", "out").expect_err(name);
            assert!(
                refused.message.contains("names no file"),
                "{}",
                refused.message
            );
        }
    }
}
