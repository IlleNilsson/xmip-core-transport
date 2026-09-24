//! One Stream as it arrived, and where it came from.

/// What a transport hands back from `receive`.
///
/// `origin_uri` is historical fact and never changes, per ADR-0013. It says
/// where the bytes came from, not where they are now — a file that is later
/// consumed by being moved still arrived from the path it was read at.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Arrived {
    pub origin_uri: String,
    pub bytes: Vec<u8>,
}

impl Arrived {
    #[must_use]
    pub fn new(origin_uri: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            origin_uri: origin_uri.into(),
            bytes: bytes.into(),
        }
    }
}

/// The first of `arrivals`, or the failure `missing` says when there is
/// none: what a far end hands back from a receive that may come up empty —
/// a session's next publish, the rows a read-back found. Nine far ends
/// wrote `.into_iter().next().ok_or_else(..)` until 2026-09-24.
///
/// # Errors
/// Where nothing arrived: a peer that broke the exchange, which saying
/// again will not mend.
pub fn next_arrival(
    arrivals: impl IntoIterator<Item = Arrived>,
    missing: &str,
) -> crate::error::Result<Arrived> {
    arrivals
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::protocol_error(missing))
}

/// The one of `arrivals`, where exactly one was due, or the failure that
/// says how many came instead: `"<did> <count> messages, not one"`, where
/// `did` is what brought them — `"collected"`, `"the client appended"`.
/// POP3's and IMAP's far ends each wrote it until 2026-09-24.
///
/// # Errors
/// Where none or more than one arrived.
pub fn one_arrival(mut arrivals: Vec<Arrived>, did: &str) -> crate::error::Result<Arrived> {
    match arrivals.len() {
        1 => Ok(arrivals.remove(0)),
        count => Err(crate::error::protocol_error(format!(
            "{did} {count} messages, not one"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_arrival_is_the_one_or_says_how_many_came() {
        let one = vec![Arrived::new("a://1", vec![1])];
        assert_eq!(one_arrival(one, "collected").expect("one").bytes, [1]);
        let error = one_arrival(Vec::new(), "collected").expect_err("none");
        assert_eq!(error.message, "collected 0 messages, not one");
    }

    #[test]
    fn the_next_arrival_is_the_first_or_says_what_is_missing() {
        let two = vec![
            Arrived::new("a://1", vec![1]),
            Arrived::new("a://2", vec![2]),
        ];
        assert_eq!(
            next_arrival(two, "none").expect("first").origin_uri,
            "a://1"
        );
        let error = next_arrival(None, "the client closed without one").expect_err("none");
        assert_eq!(error.message, "the client closed without one");
    }

    #[test]
    fn an_arrival_carries_its_origin_with_it() {
        let arrived = Arrived::new("file:///in/order.edi", b"ISA*00*".to_vec());

        assert_eq!(arrived.origin_uri, "file:///in/order.edi");
        assert_eq!(arrived.bytes, b"ISA*00*");
    }
}
