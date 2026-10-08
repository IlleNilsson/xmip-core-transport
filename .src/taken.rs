//! What a far end took whole, and where it came from.

use std::net::SocketAddr;

/// The bytes one far end took in one exchange, already answered as its
/// protocol answers: what a loopback round ([`crate::Loopback::round`])
/// and every far end a technology stands up hand back to be judged.
///
/// Not an [`crate::Arrived`]. A far end is the other side of a send, held
/// whole to compare with what was sent; a Receive Location's arrival is a
/// Stream pulled in chunks and acknowledged after its receive cycle. A far
/// end built on its technology's own receive takes each arrival with
/// [`crate::Arrived::taken`], which keeps what the arrival observed of its
/// sender ([`Taken::observation`]).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Taken {
    pub origin_uri: String,
    pub bytes: Vec<u8>,
    /// What the transport observed of the sender, each under its
    /// `context::property` name ([`crate::arrival_identity`]).
    pub observed: Vec<(String, String)>,
}

impl Taken {
    #[must_use]
    pub fn new(origin_uri: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            origin_uri: origin_uri.into(),
            bytes: bytes.into(),
            observed: Vec::new(),
        }
    }

    /// The same, having observed `value` under `name`.
    #[must_use]
    pub fn observing(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.observed.push((name.into(), value.into()));
        self
    }

    /// The same, from the socket peer `peer` ([`crate::arrival_identity::peer`]).
    #[must_use]
    pub fn from_peer(self, peer: SocketAddr) -> Self {
        let (name, value) = crate::arrival_identity::peer(peer);
        self.observing(name, value)
    }

    /// The same, from the hardware address `mac`
    /// ([`crate::arrival_identity::peer_mac`]).
    #[must_use]
    pub fn from_peer_mac(self, mac: &impl ToString) -> Self {
        let (name, value) = crate::arrival_identity::peer_mac(mac);
        self.observing(name, value)
    }

    /// What was observed under `name`, the first where it came twice.
    #[must_use]
    pub fn observation(&self, name: &str) -> Option<&str> {
        self.observed
            .iter()
            .find(|(observed, _)| observed == name)
            .map(|(_, value)| value.as_str())
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
        assert!(taken.observed.is_empty());
    }
}
