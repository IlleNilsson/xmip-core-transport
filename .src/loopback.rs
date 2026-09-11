//! A transport that stands at both ends of one exchange on this machine.
//!
//! ADR-0028 clause 5: the far end is Xmip. Every transport that ships both a
//! server's worth of protocol and a client can be its own counterparty, and
//! the Playground exercises it that way — nothing external is stood up. What
//! that takes is one dance per protocol: stand up the far end, learn where
//! it listens, send from a fresh near end, take the one arrival. Until
//! 2026-09-11 that dance was written in the Playground, once per transport,
//! forty-two times over. It belongs with the protocol (ADR-0051): a
//! technology implements [`Loopback`], and whoever drives it — the
//! Playground, the technology's own tests — calls [`Loopback::round`].
//!
//! The ceiling and the refusals live here too, because they are facts about
//! the protocol and are written where they come from (ADR-0028, amendment
//! 2026-09-09): one datagram carries 65 507 bytes, a mail path canonicalises
//! line endings, an MLLP block cannot hold its own terminator.

use std::net::TcpStream;
use std::time::Duration;

use crate::arrived::Arrived;
use crate::error::{Result, protocol_error};
use crate::protocol::Transport;

/// How long a far end waits on its near end before the round is judged
/// rather than waited on: a lost datagram, a peer that never connects, a
/// broker gone quiet. Two seconds is long enough for loopback and short
/// enough that a matrix of hundreds of pairs stays a test.
pub const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(2);

/// One far end, stood up and waiting for its one exchange.
pub trait FarEnd: Send {
    /// Where the near end sends: an address in the protocol's own terms.
    fn address(&self) -> &str;

    /// Wait for the one exchange and hand back what arrived. Consumes the far
    /// end: one exchange is what it was stood up for.
    ///
    /// # Errors
    /// Where nothing arrived before the timeout, or what arrived could not be
    /// read.
    fn take_one(self: Box<Self>) -> Result<Arrived>;
}

/// A transport that can be both ends of one exchange on this machine.
///
/// Implemented by a technology on its transport type, configured as the
/// loopback pair wants it — an ephemeral local port, the loopback timeout —
/// and handed out by the technology's `loopback()` constructor. It is a
/// [`Transport`], so its name is the transport's name.
pub trait Loopback: Transport + Send + Sync {
    /// The largest payload this protocol carries whole in one round, or
    /// `None` when it carries any. A fact about the protocol, never a number
    /// chosen to make a test pass.
    fn ceiling(&self) -> Option<usize> {
        None
    }

    /// Why this protocol cannot carry `payload` as it is, or `None` when it
    /// can. A ceiling is about size; this is about content. A refusal is
    /// judged one-sided: the transport declares the shape it does not carry
    /// rather than changing the bytes and calling that delivered.
    fn refuses(&self, payload: &[u8]) -> Option<String> {
        let _ = payload;
        None
    }

    /// Stand up the far end and learn where it listens.
    ///
    /// # Errors
    /// Where the far end could not be bound.
    fn far_end(&self) -> Result<Box<dyn FarEnd>>;

    /// Send `payload` from a fresh near end to `address`.
    ///
    /// # Errors
    /// Where the near end could not reach the far end or was refused.
    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()>;

    /// Unblock a far end whose near end failed before it connected — an
    /// ephemeral port exhausted, a refused connect under load — so the round
    /// is judged rather than waited on. The default pokes a listening socket
    /// with a throwaway connect, which is what every socket protocol needs and
    /// what a datagram or in-process protocol, with its own timeout, ignores.
    fn unblock(&self, address: &str) {
        drop(TcpStream::connect(address));
    }

    /// One round: stand up the far end, send from the near end on this
    /// thread while the far end takes on another, and return what arrived.
    /// A protocol whose two ends do not need two threads — a directory, an
    /// in-process bus — overrides this and does its round in order.
    ///
    /// # Errors
    /// Where either end failed, with which one.
    fn round(&self, payload: &[u8]) -> Result<Arrived> {
        let far = self.far_end()?;
        let address = far.address().to_string();
        let taking = std::thread::spawn(move || far.take_one());
        let sent = self.send_to(&address, payload);
        if sent.is_err() {
            self.unblock(&address);
        }
        let taken = taking
            .join()
            .map_err(|_| protocol_error("the far end's thread panicked"))?;
        sent.map_err(|error| protocol_error(format!("send failed: {error}")))?;
        taken.map_err(|error| protocol_error(format!("take failed: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// An in-process protocol: the far end is a mailbox, the near end drops
    /// bytes in it, and the round takes them out in order on one thread.
    struct Mailbox(Arc<Mutex<Option<Vec<u8>>>>);

    struct Slot(Arc<Mutex<Option<Vec<u8>>>>);

    impl FarEnd for Slot {
        fn address(&self) -> &'static str {
            "mailbox"
        }

        fn take_one(self: Box<Self>) -> Result<Arrived> {
            let deadline = std::time::Instant::now() + LOOPBACK_TIMEOUT;
            loop {
                if let Some(bytes) = self.0.lock().expect("lock").take() {
                    return Ok(Arrived::new("mailbox://one", bytes));
                }
                if std::time::Instant::now() > deadline {
                    return Err(protocol_error("nothing was dropped in the mailbox"));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    impl Transport for Mailbox {
        fn name(&self) -> &'static str {
            "mailbox"
        }

        fn directions(&self) -> crate::direction::Directions {
            crate::direction::Directions::BOTH
        }

        fn receive(&self) -> Result<Vec<Arrived>> {
            Ok(self
                .0
                .lock()
                .expect("lock")
                .take()
                .map(|bytes| Arrived::new("mailbox://one", bytes))
                .into_iter()
                .collect())
        }

        fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
            self.send_to(target, bytes)
        }
    }

    impl Loopback for Mailbox {
        fn ceiling(&self) -> Option<usize> {
            Some(8)
        }

        fn far_end(&self) -> Result<Box<dyn FarEnd>> {
            Ok(Box::new(Slot(Arc::clone(&self.0))))
        }

        fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
            if address != "mailbox" {
                return Err(protocol_error("no such mailbox"));
            }
            if payload.len() > 8 {
                return Err(protocol_error("over the mailbox's ceiling"));
            }
            *self.0.lock().expect("lock") = Some(payload.to_vec());
            Ok(())
        }
    }

    #[test]
    fn a_round_sends_on_one_thread_and_takes_on_another() {
        let mailbox = Mailbox(Arc::new(Mutex::new(None)));
        let arrived = mailbox.round(b"ping").expect("round");
        assert_eq!(arrived.bytes, b"ping");
        assert_eq!(arrived.origin_uri, "mailbox://one");
    }

    #[test]
    fn a_failed_send_is_judged_and_names_the_send() {
        let mailbox = Mailbox(Arc::new(Mutex::new(None)));
        let error = mailbox.round(b"over the top").expect_err("refused");
        assert!(error.message.starts_with("send failed:"), "{error}");
        assert_eq!(mailbox.ceiling(), Some(8));
        assert!(mailbox.refuses(b"x").is_none());
        assert_eq!(mailbox.name(), "mailbox");
    }
}
