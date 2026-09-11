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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrival_carries_its_origin_with_it() {
        let arrived = Arrived::new("file:///in/order.edi", b"ISA*00*".to_vec());

        assert_eq!(arrived.origin_uri, "file:///in/order.edi");
        assert_eq!(arrived.bytes, b"ISA*00*");
    }
}
