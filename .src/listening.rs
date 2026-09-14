//! A bound listener waiting for its one connection: the far end of every
//! TCP technology's loopback round (ADR-0051).
//!
//! Thirty-three technologies each declared this struct, its address and
//! the `far_end` that boxes it before 2026-09-14, and what two technologies
//! both need is shared through the capability (ADR-0044). What differs
//! between them is what happens once the connection is accepted — a
//! session served to its one INSERT, a STOMP client seen to its
//! DISCONNECT — and that is [`Accepting::take_one`], which the technology
//! keeps.

use std::net::TcpListener;

use crate::arrived::Arrived;
use crate::error::Result;
use crate::loopback::FarEnd;
use crate::socket;

/// What a technology does with the one exchange its far end accepts.
pub trait Accepting: Send {
    /// Accept on `listener`, see the exchange through, and hand back what
    /// arrived.
    ///
    /// # Errors
    /// Where the connection could not be accepted, nothing arrived before
    /// the timeout, or the exchange was not the one the protocol expects.
    fn take_one(&self, listener: &TcpListener) -> Result<Arrived>;
}

/// A bound listener waiting for its one connection.
pub struct Listening<T> {
    transport: T,
    listener: TcpListener,
    address: String,
}

impl<T> Listening<T> {
    /// `listener`, already bound at `address`, waiting for `transport` to
    /// take what connects.
    #[must_use]
    pub fn new(transport: T, listener: TcpListener, address: String) -> Self {
        Self {
            transport,
            listener,
            address,
        }
    }

    /// Bind at `bind` and wait there.
    ///
    /// # Errors
    /// Where the address is taken, malformed, or not permitted.
    pub fn bound(transport: T, bind: &str) -> Result<Self> {
        let (listener, address) = socket::bind_tcp(bind)?;
        Ok(Self::new(transport, listener, address))
    }
}

impl<T: Accepting> FarEnd for Listening<T> {
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        self.transport.take_one(&self.listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    /// A protocol that is one connection read to its end.
    struct Whole;

    impl Accepting for Whole {
        fn take_one(&self, listener: &TcpListener) -> Result<Arrived> {
            let (mut stream, peer) = socket::accept_tcp(listener, Some(Duration::from_secs(2)))?;
            let mut bytes = Vec::new();
            stream
                .read_to_end(&mut bytes)
                .map_err(|e| crate::error::classify("reading", &e))?;
            Ok(Arrived::new(format!("whole://{peer}"), bytes))
        }
    }

    #[test]
    fn a_bound_listener_reports_its_address_and_takes_one_connection() {
        let far: Box<dyn FarEnd> = Box::new(Listening::bound(Whole, "127.0.0.1:0").expect("bind"));
        let address = far.address().to_string();
        assert!(address.starts_with("127.0.0.1:") && !address.ends_with(":0"));
        let sending = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(&address).expect("connect");
            stream.write_all(b"one exchange").expect("write");
        });
        let arrived = far.take_one().expect("take");
        sending.join().expect("thread");
        assert_eq!(arrived.bytes, b"one exchange");
        assert!(arrived.origin_uri.starts_with("whole://127.0.0.1:"));
    }
}
