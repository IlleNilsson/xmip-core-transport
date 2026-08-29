//! Streams that arrive as datagrams. One datagram is one Stream.
//!
//! There is no reply channel and no delivery guarantee, which makes this the
//! clearest case of a transport that **cannot answer**: a Contract failure here
//! is audited and nothing more. HTTP is the same shape with a reply channel, and
//! the two behave differently at the gate for exactly that reason.

use std::net::UdpSocket;

use crate::arrived::Arrived;
use crate::direction::Directions;
use crate::error::{classify, Result};
use crate::protocol::Transport;

/// The largest a UDP payload can be over IPv4: 65535 less the 8-byte UDP header
/// and the 20-byte IP header.
const MAX_DATAGRAM: usize = 65_507;

pub struct UdpTransport {
    bind: String,
    max_datagram: usize,
}

impl UdpTransport {
    #[must_use]
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            max_datagram: MAX_DATAGRAM,
        }
    }
}

impl Transport for UdpTransport {
    fn name(&self) -> &'static str {
        "udp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let socket = UdpSocket::bind(&self.bind).map_err(|e| classify("binding the socket", &e))?;

        let mut buffer = vec![0u8; self.max_datagram];
        let (read, peer) = socket
            .recv_from(&mut buffer)
            .map_err(|e| classify("receiving a datagram", &e))?;

        buffer.truncate(read);

        Ok(vec![Arrived::new(format!("udp://{peer}"), buffer)])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|e| classify("binding the sending socket", &e))?;

        socket
            .send_to(bytes, target)
            .map_err(|e| classify("sending a datagram", &e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_round_trip_carries_bytes_and_peer() {
        // Bound here rather than inside the transport, because the receiver has
        // to be listening before the datagram is sent. UDP drops it silently
        // otherwise, and the test would hang rather than fail.
        let socket = UdpSocket::bind("127.0.0.1:0").expect("binding");
        let address = socket.local_addr().expect("reading the address").to_string();

        let sender = std::thread::spawn(move || {
            UdpTransport::new("127.0.0.1:0")
                .send(&address, b"hello over udp")
                .expect("sending");
        });

        let mut buffer = vec![0u8; MAX_DATAGRAM];
        let (read, peer) = socket.recv_from(&mut buffer).expect("receiving");
        sender.join().expect("the sending thread panicked");

        buffer.truncate(read);

        assert_eq!(buffer, b"hello over udp");
        assert!(peer.to_string().starts_with("127.0.0.1:"));
    }

    #[test]
    fn a_datagram_socket_has_no_artefact_to_claim() {
        assert!(UdpTransport::new("127.0.0.1:0").claims().is_none());
    }
}
