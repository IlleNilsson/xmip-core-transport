//! A bound listener waiting for its one connection: the far end of every
//! TCP technology's loopback round (ADR-0051).
//!
//! Thirty-three technologies each declared this struct, its address and
//! the `far_end` that boxes it before 2026-09-14, and eighteen more still
//! wrote their own until 2026-09-24 — a `Serving` over a cloud session, a
//! `Collecting` maildrop, an SSH `Inbox` — and what two technologies both
//! need is shared through the capability (ADR-0044). What differs between
//! them is what happens once the connection is accepted — a session served
//! to its one INSERT, a STOMP client seen to its DISCONNECT — and that is
//! [`Accepting::take_one`], which the technology keeps: on its transport
//! type, or as a closure over the session it serves.

use std::net::TcpListener;

use crate::arrived::Arrived;
use crate::error::Result;
use crate::loopback::FarEnd;
use crate::socket;

/// What a technology does with the one exchange its far end accepts.
///
/// Implemented by a technology on its transport type, or by any closure
/// that takes the listener: a far end is stood up for one exchange, so the
/// taking consumes what took.
pub trait Accepting: Send {
    /// Accept on `listener`, see the exchange through, and hand back what
    /// arrived.
    ///
    /// # Errors
    /// Where the connection could not be accepted, nothing arrived before
    /// the timeout, or the exchange was not the one the protocol expects.
    fn take_one(self, listener: &TcpListener) -> Result<Arrived>;
}

impl<F> Accepting for F
where
    F: FnOnce(&TcpListener) -> Result<Arrived> + Send,
{
    fn take_one(self, listener: &TcpListener) -> Result<Arrived> {
        self(listener)
    }
}

/// A bound listener waiting for its one connection.
pub struct Listening<T> {
    taking: T,
    listener: TcpListener,
    address: String,
}

impl<T> Listening<T> {
    /// `bound`, the listener and the address it is bound at — what
    /// [`socket::bind_tcp`] and every technology's `bind` hand back —
    /// waiting for `taking` to take what connects.
    #[must_use]
    pub fn new(taking: T, bound: (TcpListener, String)) -> Self {
        let (listener, address) = bound;
        Self {
            taking,
            listener,
            address,
        }
    }

    /// Bind at `bind` and wait there.
    ///
    /// # Errors
    /// Where the address is taken, malformed, or not permitted.
    pub fn bound(taking: T, bind: &str) -> Result<Self> {
        Ok(Self::new(taking, socket::bind_tcp(bind)?))
    }
}

impl<T: Accepting> FarEnd for Listening<T> {
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        let Self {
            taking, listener, ..
        } = *self;
        taking.take_one(&listener)
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
        fn take_one(self, listener: &TcpListener) -> Result<Arrived> {
            let (mut stream, peer) = socket::accept_tcp(listener, Some(Duration::from_secs(2)))?;
            let mut bytes = Vec::new();
            stream
                .read_to_end(&mut bytes)
                .map_err(|e| crate::error::classify("reading", &e))?;
            Ok(Arrived::new(format!("whole://{peer}"), bytes))
        }
    }

    fn sent(address: String, bytes: &'static [u8]) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(&address).expect("connect");
            stream.write_all(bytes).expect("write");
        })
    }

    #[test]
    fn a_bound_listener_reports_its_address_and_takes_one_connection() {
        let far: Box<dyn FarEnd> = Box::new(Listening::bound(Whole, "127.0.0.1:0").expect("bind"));
        let address = far.address().to_string();
        assert!(address.starts_with("127.0.0.1:") && !address.ends_with(":0"));
        let sending = sent(address, b"one exchange");
        let arrived = far.take_one().expect("take");
        sending.join().expect("thread");
        assert_eq!(arrived.bytes, b"one exchange");
        assert!(arrived.origin_uri.starts_with("whole://127.0.0.1:"));
    }

    #[test]
    fn a_closure_takes_the_one_connection_and_owns_what_it_serves() {
        let mut served = Vec::new();
        let taking = move |listener: &TcpListener| {
            let (mut stream, _) = socket::accept_tcp(listener, Some(Duration::from_secs(2)))?;
            stream
                .read_to_end(&mut served)
                .map_err(|e| crate::error::classify("reading", &e))?;
            Ok(Arrived::new("served://one", served))
        };
        let far = Box::new(Listening::bound(taking, "127.0.0.1:0").expect("bind"));
        let sending = sent(far.address().to_string(), b"by closure");
        let arrived = far.take_one().expect("take");
        sending.join().expect("thread");
        assert_eq!(
            arrived,
            Arrived::new("served://one", b"by closure".to_vec())
        );
    }
}
