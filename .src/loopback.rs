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

use std::time::Duration;

use crate::arrived::Arrived;
use crate::error::{Result, protocol_error};
use crate::protocol::Transport;

/// How long a far end waits on its near end before the round is judged
/// rather than waited on: a lost datagram, a peer that never connects, a
/// broker gone quiet. Two seconds is long enough for loopback and short
/// enough that a matrix of hundreds of pairs stays a test.
pub const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(2);

/// How long [`Loopback::unblock`] waits to reach a far end it is only
/// releasing. Short, because the far end gives up on its own at
/// [`LOOPBACK_TIMEOUT`] and the poke is a courtesy that shortens the wait.
pub const UNBLOCK_TIMEOUT: Duration = Duration::from_millis(250);

/// One far end, stood up and waiting for its one exchange.
pub trait FarEnd: Send {
    /// Where the near end sends: an address in the protocol's own terms.
    fn address(&self) -> &str;

    /// Whether this far end reads a datagram socket: it waits with its own
    /// timeout and nothing listens at its address, so a round whose near
    /// end failed leaves it to time out rather than poke a port that is
    /// not its own. `bound::Bound` says yes; every other far end, no.
    fn datagram(&self) -> bool {
        false
    }

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

    /// Why this machine cannot stand both ends — an OS object it lacks, a
    /// privilege it does not grant — or `None` when it can. Not a failure:
    /// whoever drives the round judges it one-sided and says why, rather
    /// than red for a fact about the machine.
    fn unavailable(&self) -> Option<String> {
        None
    }

    /// Whether the two ends exchange in order on one thread: the far end
    /// answers as the near end sends — a bus, a line with one master, a
    /// radio held in process, a directory, a file — so there is nothing to
    /// wait on and two threads would only race. A protocol that does says
    /// so here, in its own words, and [`Loopback::round`] goes
    /// [`Loopback::round_in_order`]. Sixteen technologies overrode `round`
    /// to say it until 2026-09-25.
    fn exchanges_in_order(&self) -> bool {
        false
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
    /// is judged rather than waited on. The default knows the three kinds of
    /// address a far end has: a TCP listener is poked with a throwaway
    /// connect; a protocol that exchanges in order never waits, so there is
    /// nothing to release; and a datagram far end, which reads with its own
    /// timeout, is never handed here — [`Loopback::round`] asks the far end
    /// ([`FarEnd::datagram`]) before it would. A technology overrides this
    /// only where its far end is released some other way: a path to connect
    /// to, a frame that ends a transfer. Twenty-six technologies overrode it
    /// with nothing until 2026-09-25.
    ///
    /// The poke is bounded and short. It was a bare `TcpStream::connect`
    /// until 2026-09-21, and the case it was written for is the case it could
    /// not survive: on a machine out of ephemeral ports the poke itself waited
    /// on Windows' own twenty-one-second SYN schedule, so a round that failed
    /// in milliseconds was judged twenty-three seconds later. A far end bounds
    /// its own accept now (`socket::accept_tcp`), so the poke only has to be
    /// quick, not certain.
    fn unblock(&self, address: &str) {
        if !self.exchanges_in_order() {
            poke(address);
        }
    }

    /// One round: stand up the far end, send from the near end on this
    /// thread while the far end takes on another, and return what arrived.
    /// A protocol whose two ends do not need two threads — a directory, an
    /// in-process bus — says so in [`Loopback::exchanges_in_order`], and the
    /// round is [`Loopback::round_in_order`].
    ///
    /// # Errors
    /// Where either end failed, with which one.
    fn round(&self, payload: &[u8]) -> Result<Arrived> {
        if self.exchanges_in_order() {
            return self.round_in_order(payload);
        }
        let far = self.far_end()?;
        let address = far.address().to_string();
        let datagram = far.datagram();
        let (sent, taken) = both_ends(
            move || far.take_one(),
            || self.send_to(&address, payload),
            || {
                if !datagram {
                    self.unblock(&address);
                }
            },
        );
        sent.map_err(|error| error.at("send failed"))?;
        taken.map_err(|error| error.at("take failed"))
    }

    /// One round in order on one thread: the send goes first and the take
    /// finds what it left. For a protocol whose far end answers as the near
    /// end sends — a bus, a line with one master, a directory, a file — so
    /// there is nothing to wait on and two threads would only race.
    /// [`Loopback::round`] is this where [`Loopback::exchanges_in_order`]
    /// says so. Seventeen technologies each wrote this before 2026-09-14
    /// (ADR-0044).
    ///
    /// # Errors
    /// Where either end failed, or what was taken back is not what was
    /// sent.
    fn round_in_order(&self, payload: &[u8]) -> Result<Arrived> {
        let far = self.far_end()?;
        self.send_to(far.address(), payload)?;
        let arrived = far.take_one()?;
        if arrived.bytes != payload {
            return Err(protocol_error("sent, but what was taken back differs"));
        }
        Ok(arrived)
    }
}

/// Poke a listening socket at `address` with a throwaway connect, bounded
/// by [`UNBLOCK_TIMEOUT`], so a far end waiting on a near end that failed
/// is judged rather than waited on. What [`Loopback::unblock`] does by
/// default, and what a far end that stands a second listener of its own —
/// a subscription's endpoint, a webhook — does to it. Written four times,
/// three with the number in it, until 2026-09-24.
pub fn poke(address: &str) {
    drop(crate::socket::connect_tcp(address, Some(UNBLOCK_TIMEOUT)));
}

/// Both ends of one exchange: `take` on a thread of its own, `give` on this
/// one, and `unblock` when the giving failed, so the taker is released
/// rather than waited on. Hands back what each end came to, for the caller
/// to judge in its own words. [`Loopback::round`] is this over a far end
/// and a near end; a far end that delivers onward — SNS to its
/// subscription, Event Grid to its webhook — is this again, inside, and so
/// is the Playground filing an archive through a far end it serves.
///
/// A taking thread that panicked is a failed take.
pub fn both_ends<T: Send, U, E>(
    take: impl FnOnce() -> Result<T> + Send,
    give: impl FnOnce() -> std::result::Result<U, E>,
    unblock: impl FnOnce(),
) -> (std::result::Result<U, E>, Result<T>) {
    std::thread::scope(|scope| {
        let taking = scope.spawn(take);
        let given = give();
        if given.is_err() {
            unblock();
        }
        let taken = taking
            .join()
            .unwrap_or_else(|_| Err(protocol_error("the far end's thread panicked")));
        (given, taken)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    type Letter = Arc<Mutex<Option<Vec<u8>>>>;

    /// An in-process protocol: the far end is a mailbox and the near end
    /// drops bytes in it. It declares whether it exchanges in order and
    /// whether its far end reads a datagram, and records an unblock.
    struct Mailbox {
        letter: Letter,
        in_order: bool,
        datagram: bool,
        unblocked: AtomicBool,
    }

    fn mailbox(in_order: bool, datagram: bool) -> Mailbox {
        Mailbox {
            letter: Arc::new(Mutex::new(None)),
            in_order,
            datagram,
            unblocked: AtomicBool::new(false),
        }
    }

    struct Slot {
        letter: Letter,
        datagram: bool,
    }

    impl FarEnd for Slot {
        fn address(&self) -> &'static str {
            "mailbox"
        }

        fn datagram(&self) -> bool {
            self.datagram
        }

        fn take_one(self: Box<Self>) -> Result<Arrived> {
            let deadline = std::time::Instant::now() + LOOPBACK_TIMEOUT;
            loop {
                if let Some(bytes) = self.letter.lock().expect("lock").take() {
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
                .letter
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

        fn exchanges_in_order(&self) -> bool {
            self.in_order
        }

        fn far_end(&self) -> Result<Box<dyn FarEnd>> {
            Ok(Box::new(Slot {
                letter: Arc::clone(&self.letter),
                datagram: self.datagram,
            }))
        }

        fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
            if address != "mailbox" {
                return Err(protocol_error("no such mailbox"));
            }
            if payload.len() > 8 {
                return Err(protocol_error("over the mailbox's ceiling"));
            }
            *self.letter.lock().expect("lock") = Some(payload.to_vec());
            Ok(())
        }

        fn unblock(&self, _address: &str) {
            self.unblocked.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_round_sends_on_one_thread_and_takes_on_another() {
        let mailbox = mailbox(false, false);
        let arrived = mailbox.round(b"ping").expect("round");
        assert_eq!(arrived.bytes, b"ping");
        assert_eq!(arrived.origin_uri, "mailbox://one");
    }

    #[test]
    fn a_round_in_order_sends_then_takes_and_checks_what_came_back() {
        let mailbox = mailbox(false, false);
        let arrived = mailbox.round_in_order(b"pong").expect("round");
        assert_eq!(arrived.bytes, b"pong");
        let error = mailbox
            .round_in_order(b"over the top")
            .expect_err("refused");
        assert!(error.message.contains("ceiling"), "{error}");
    }

    #[test]
    fn a_failed_send_is_judged_names_the_send_and_unblocks_the_far_end() {
        let mailbox = mailbox(false, false);
        let error = mailbox.round(b"over the top").expect_err("refused");
        assert!(error.message.starts_with("send failed:"), "{error}");
        assert!(mailbox.unblocked.load(Ordering::SeqCst));
        assert_eq!(mailbox.ceiling(), Some(8));
        assert!(mailbox.refuses(b"x").is_none());
        assert_eq!(mailbox.name(), "mailbox");
    }

    #[test]
    fn a_protocol_that_exchanges_in_order_rounds_in_order_and_never_unblocks() {
        let mailbox = mailbox(true, false);
        assert_eq!(mailbox.round(b"ping").expect("round").bytes, b"ping");
        let error = mailbox.round(b"over the top").expect_err("refused");
        assert!(!error.message.starts_with("send failed:"), "{error}");
        assert!(!mailbox.unblocked.load(Ordering::SeqCst));
    }

    #[test]
    fn a_datagram_far_end_is_left_to_its_own_timeout() {
        let mailbox = mailbox(false, true);
        let error = mailbox.round(b"over the top").expect_err("refused");
        assert!(error.message.starts_with("send failed:"), "{error}");
        assert!(!mailbox.unblocked.load(Ordering::SeqCst));
    }
}
