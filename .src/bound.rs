//! A bound datagram socket waiting for its one exchange: the far end of
//! every UDP technology's loopback round (ADR-0051).
//!
//! UDP, `BACnet`, CoAP, DDS, DHCP, KNX, mDNS, SSDP and SNMP each declared this
//! struct — the socket, the address it is bound at, the transport to read it
//! with — and its `FarEnd`, until 2026-09-24. Bound before the near end
//! fires, or the first datagram is gone. What the far end does with what it
//! reads — a datagram taken whole, POSTs answered 2.04 until an empty one,
//! an announcement followed by queries — is [`Reading::take_one`], which the
//! technology keeps, as a TCP technology keeps
//! [`crate::listening::Accepting::take_one`].

use std::net::UdpSocket;

use crate::arrived::Arrived;
use crate::error::Result;
use crate::loopback::FarEnd;

/// What a technology does with the one exchange its far end reads.
///
/// Implemented by a technology on its transport type, or by any closure
/// that takes the socket: a far end is stood up for one exchange, so the
/// taking consumes what took.
pub trait Reading: Send {
    /// Read the one exchange from `socket`, answer it as the protocol
    /// answers, and hand back what arrived.
    ///
    /// # Errors
    /// Where nothing arrived before the timeout, or what arrived was not
    /// the exchange the protocol expects.
    fn take_one(self, socket: &UdpSocket) -> Result<Arrived>;
}

impl<F> Reading for F
where
    F: FnOnce(&UdpSocket) -> Result<Arrived> + Send,
{
    fn take_one(self, socket: &UdpSocket) -> Result<Arrived> {
        self(socket)
    }
}

/// A bound socket waiting for its one exchange.
pub struct Bound<T> {
    taking: T,
    socket: UdpSocket,
    address: String,
}

impl<T> Bound<T> {
    /// `bound`, the socket and the address it is bound at — what
    /// [`crate::socket::bind_udp`] and every technology's `bind` hand back —
    /// read by `taking` for the one exchange.
    #[must_use]
    pub fn new(taking: T, bound: (UdpSocket, String)) -> Self {
        let (socket, address) = bound;
        Self {
            taking,
            socket,
            address,
        }
    }
}

impl<T: Reading> FarEnd for Bound<T> {
    fn address(&self) -> &str {
        &self.address
    }

    /// A datagram socket reads with its own timeout, and nothing listens at
    /// its address for a round to poke.
    fn datagram(&self) -> bool {
        true
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        let Self { taking, socket, .. } = *self;
        taking.take_one(&socket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classify;
    use crate::socket;
    use std::time::Duration;

    #[test]
    fn a_bound_socket_reports_its_address_and_reads_one_datagram() {
        let bound = socket::bind_udp("127.0.0.1:0", Some(Duration::from_secs(2))).expect("bind");
        let far = Box::new(Bound::new(
            |socket: &UdpSocket| {
                let mut buffer = [0u8; 64];
                let (read, peer) = socket
                    .recv_from(&mut buffer)
                    .map_err(|e| classify("receiving", &e))?;
                Ok(Arrived::new(format!("udp://{peer}"), &buffer[..read]))
            },
            bound,
        ));
        let address = far.address().to_string();
        assert!(far.datagram());
        assert!(address.starts_with("127.0.0.1:") && !address.ends_with(":0"));
        let near = UdpSocket::bind("127.0.0.1:0").expect("near");
        near.send_to(b"one datagram", &address).expect("send");
        let arrived = far.take_one().expect("take");
        assert_eq!(arrived.bytes, b"one datagram");
        assert!(arrived.origin_uri.starts_with("udp://127.0.0.1:"));
    }
}
