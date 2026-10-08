//! What a protocol delivered beside the bytes: its headers, under the
//! protocol's word.
//!
//! An HTTP request's fields, a STOMP frame's headers, an AMQP header table,
//! a Kafka record's headers, NATS headers, a Pub/Sub message's attributes:
//! every technology whose protocol carries named values beside the payload
//! hands them with its [`crate::Arrived`] through this, and writes nothing
//! into a context. The runtime writes them into the Message Context at
//! arrival, each as `<protocol>.header.<name>` through
//! `context::property::header`, case folded where the protocol folds it
//! (ADR-0046, amendment 2026-09-25, later). A header is handed as the
//! protocol delivered it: its name as written, its value text, or bytes
//! where the protocol's value is octets that are not UTF-8.

use xcore::ScalarValue;

/// One protocol's headers on one arrival, in the order they came.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Headers {
    protocol: String,
    fields: Vec<(String, ScalarValue)>,
}

impl Headers {
    /// No headers yet, of `protocol` — the word for what is spoken on the
    /// wire, as a URI scheme gives it: `http`, `amqp`, `kafka`, `nats`.
    #[must_use]
    pub fn of(protocol: &str) -> Self {
        Self {
            protocol: protocol.to_string(),
            fields: Vec::new(),
        }
    }

    /// These headers and `fields`, each value text.
    #[must_use]
    pub fn text<N, V>(mut self, fields: impl IntoIterator<Item = (N, V)>) -> Self
    where
        N: Into<String>,
        V: Into<String>,
    {
        self.fields.extend(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), ScalarValue::Text(value.into()))),
        );
        self
    }

    /// These headers and `fields`, each value octets: text where they are
    /// UTF-8, bytes where they are not, which a filter refuses to read as
    /// text (ADR-0046, amendment 2026-09-24).
    #[must_use]
    pub fn octets<N>(mut self, fields: impl IntoIterator<Item = (N, Vec<u8>)>) -> Self
    where
        N: Into<String>,
    {
        self.fields.extend(fields.into_iter().map(|(name, value)| {
            (
                name.into(),
                match String::from_utf8(value) {
                    Ok(text) => ScalarValue::Text(text),
                    Err(octets) => ScalarValue::Binary(octets.into_bytes()),
                },
            )
        }));
        self
    }

    /// The protocol's word.
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// Each header, its name as written and its value, in arrival order.
    #[must_use]
    pub fn fields(&self) -> &[(String, ScalarValue)] {
        &self.fields
    }

    /// Whether nothing was handed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// The protocol's word and the headers, taken apart.
    #[must_use]
    pub fn into_parts(self) -> (String, Vec<(String, ScalarValue)>) {
        (self.protocol, self.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_octets_are_kept_in_order_as_the_protocol_wrote_them() {
        let headers = Headers::of("kafka")
            .text([("Trace-Id", "a1")])
            .octets([("trace-id", b"b2".to_vec()), ("raw", vec![0xff, 0x00])]);
        assert_eq!(headers.protocol(), "kafka");
        assert_eq!(
            headers.fields(),
            [
                ("Trace-Id".to_string(), ScalarValue::Text("a1".into())),
                ("trace-id".to_string(), ScalarValue::Text("b2".into())),
                ("raw".to_string(), ScalarValue::Binary(vec![0xff, 0x00])),
            ]
        );
        assert!(Headers::default().is_empty());
    }
}
