//! What a far end took whole, and where it came from.

/// The bytes one far end took in one exchange, already answered as its
/// protocol answers: what a loopback round ([`crate::Loopback::round`])
/// and every far end a technology stands up hand back to be judged.
///
/// Not an [`crate::Arrived`]. A far end is the other side of a send, held
/// whole to compare with what was sent; a Receive Location's arrival is a
/// Stream pulled in chunks and acknowledged after its receive cycle. A far
/// end built on its technology's own receive takes each arrival with
/// [`crate::Arrived::taken`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Taken {
    pub origin_uri: String,
    pub bytes: Vec<u8>,
}

impl Taken {
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
    fn what_a_far_end_took_carries_its_origin() {
        let taken = Taken::new("file:///in/order.edi", b"ISA*00*".to_vec());
        assert_eq!(taken.origin_uri, "file:///in/order.edi");
        assert_eq!(taken.bytes, b"ISA*00*");
    }
}
