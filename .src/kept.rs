//! A listener or a socket a Receive Location binds once and keeps: bound as
//! its transport is opened, and every receive takes from the same one.
//!
//! Until 2026-09-27 thirty technologies bound a new listener in every
//! `receive`, took one exchange and dropped it, so between two receives
//! nothing listened: a peer that connected then was refused, and a datagram
//! sent then was lost. A kept listener is still there between receives —
//! the operating system queues what connects in its backlog, and a datagram
//! in the socket's buffer — and the next receive takes it. What is kept is
//! the technology's own: a TCP listener, a datagram socket, a Unix domain
//! socket's listener, a named pipe. How it is bound is the technology's
//! `bind`, handed in; that it is bound once is here.
//!
//! **Bound at open** (the owner, 2026-11-16, *Bind at open*). The
//! technology binds it in its [`crate::Configured::listening`], which the
//! runtime calls as it opens a Receive Location's transport at startup
//! phase 9, so a node listens when it says it started, and a port that is
//! taken refuses its start. A receive takes from it ([`Kept::listener`])
//! and never binds. Until 2026-11-16 the first receive bound it, on the
//! Location's own thread after the node had started, and a caller that
//! connected the moment the start returned could be refused (open problem
//! 71).
//!
//! A loopback far end is not this: it is stood up for one exchange and
//! consumed ([`crate::listening`], [`crate::bound`]).

use std::fmt;
use std::sync::{Mutex, OnceLock};
use xcore::lock::Locked;

use crate::error::Result;

/// A listener or socket bound once and kept, with the address it is bound
/// at.
pub struct Kept<L> {
    bound: OnceLock<(L, String)>,
    /// Held while binding, so two receives racing to bind the first time
    /// bind once: the second finds the first's.
    binding: Mutex<()>,
}

impl<L> Kept<L> {
    /// Nothing bound yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            bound: OnceLock::new(),
            binding: Mutex::new(()),
        }
    }

    /// The kept listener, bound by `bind` where nothing is yet — the
    /// listener and the address it is bound at, as every technology's
    /// `bind` hands back: what its [`crate::Configured::listening`] calls
    /// as its Location's transport is opened. A bind that failed is tried
    /// again on the next call.
    ///
    /// # Errors
    /// As `bind`, where it is called and fails.
    pub fn bound(&self, bind: impl FnOnce() -> Result<(L, String)>) -> Result<&L> {
        if let Some((listener, _)) = self.bound.get() {
            return Ok(listener);
        }
        let _one = self.binding.locked();
        if let Some((listener, _)) = self.bound.get() {
            return Ok(listener);
        }
        let pair = bind()?;
        Ok(&self.bound.get_or_init(|| pair).0)
    }

    /// [`Kept::bound`] by `bind`, and where: what a technology keeping one
    /// answers as its `Configured::listening`.
    ///
    /// # Errors
    /// As `bind`, where it is called and fails.
    pub fn listening(&self, bind: impl FnOnce() -> Result<(L, String)>) -> Result<Option<&str>> {
        self.bound(bind)?;
        Ok(self.address())
    }

    /// The kept listener, as [`Kept::bound`] bound it: what every receive
    /// takes from, binding nothing.
    ///
    /// # Errors
    /// Where nothing is bound: a transport no one opened to listen
    /// ([`crate::Configured::listening`]). Permanent, since asking again
    /// binds nothing either.
    pub fn listener(&self) -> Result<&L> {
        self.bound
            .get()
            .map(|(listener, _)| listener)
            .ok_or_else(|| {
                crate::TransportError::permanent(
                    "nothing listens: a Receive Location's listener is bound as its transport is \
                 opened (Configured::listening), and this one was not",
                )
            })
    }

    /// Where the kept listener is bound: `None` until it is.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        self.bound.get().map(|(_, address)| address.as_str())
    }
}

impl<L> Default for Kept<L> {
    fn default() -> Self {
        Self::new()
    }
}

/// A copy of a transport is a configuration, not a second owner of its
/// listener: it binds its own as it is opened to listen.
impl<L> Clone for Kept<L> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl<L> fmt::Debug for Kept<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kept")
            .field("address", &self.address())
            .finish()
    }
}

/// Hold a receiver to its kept listener: `rounds` payloads sent to
/// `address` — where the receiver's `listening` bound it — one after
/// another by `send` on a thread of its own, and every one taken by
/// `receiver.receive()` on this one, in order, and accepted. A receiver
/// that bound anew would listen somewhere else, and the sends it left
/// behind would never arrive; one that dropped its listener between
/// receives would refuse the sends that came between. For a technology's
/// tests, `test-support` from a dev-dependency.
///
/// # Panics
/// Where a send or a receive failed, or what arrived is not what was sent.
#[cfg(any(test, feature = "test-support"))]
pub fn held_across_receives(
    receiver: &(impl crate::Transport + ?Sized),
    address: &str,
    rounds: usize,
    mut send: impl FnMut(&str, &[u8]) -> Result<()> + Send,
) {
    let payloads: Vec<Vec<u8>> = (0..rounds)
        .map(|round| format!("round {round}").into_bytes())
        .collect();
    std::thread::scope(|scope| {
        let sending = scope.spawn(|| {
            for payload in &payloads {
                send(address, payload).expect("sent");
            }
        });
        // A receive that found nothing within its timeout is not an error
        // (`Transport::receive`), but a few in a row is a receiver deaf to
        // what was sent.
        let mut arrived = Vec::new();
        let mut empty = 0;
        while arrived.len() < rounds {
            let taken = receiver.receive().expect("received");
            empty = if taken.is_empty() { empty + 1 } else { 0 };
            assert!(empty < 3, "receives with nothing, after {arrived:?}");
            for one in taken {
                arrived.push(one.taken().expect("accepted").bytes);
            }
        }
        sending.join().expect("the sending thread");
        assert_eq!(arrived, payloads);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const WAIT: Option<Duration> = Some(Duration::from_secs(2));

    #[test]
    fn a_listener_is_bound_once_and_takes_every_exchange() {
        let binds = AtomicUsize::new(0);
        let kept = Kept::new();
        let bind = || {
            binds.fetch_add(1, Ordering::SeqCst);
            socket::bind_tcp("127.0.0.1:0")
        };
        assert_eq!(kept.address(), None);
        let listener = kept.bound(bind).expect("bound");
        let address = kept.address().expect("an address").to_string();
        for round in 0..5u8 {
            let at = address.clone();
            let peer = std::thread::spawn(move || {
                let mut stream = socket::connect_tcp(&at, WAIT).expect("connect");
                stream.write_all(&[round]).expect("write");
            });
            let listener = kept.bound(bind).expect("the same");
            let (mut stream, _) = socket::accept_tcp(listener, WAIT).expect("accept");
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).expect("read");
            assert_eq!(byte[0], round);
            peer.join().expect("peer");
        }
        assert_eq!(binds.load(Ordering::SeqCst), 1, "bound once");
        assert!(std::ptr::eq(listener, kept.bound(bind).expect("again")));
        assert_eq!(kept.address(), Some(address.as_str()));
    }

    #[test]
    fn a_peer_that_connects_between_two_receives_is_queued_not_refused() {
        // Between two receives nothing accepts. A listener kept is still
        // listening, so the peer's connect lands in the backlog and the
        // next accept takes it; before 2026-09-27 the listener was gone and
        // the connect was refused.
        let kept = Kept::new();
        let listener = kept
            .bound(|| socket::bind_tcp("127.0.0.1:0"))
            .expect("bound");
        let address = kept.address().expect("address").to_string();
        let mut early = socket::connect_tcp(&address, WAIT).expect("not refused");
        early.write_all(b"early").expect("write");
        drop(early);
        let (mut stream, _) = socket::accept_tcp(listener, WAIT).expect("queued");
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).expect("read");
        assert_eq!(bytes, b"early");
    }

    #[test]
    fn a_failed_bind_is_tried_again_and_a_copy_binds_its_own() {
        let kept: Kept<()> = Kept::new();
        let refused = kept.bound(|| Err(crate::error::protocol_error("taken")));
        assert!(refused.is_err());
        assert_eq!(kept.address(), None);
        kept.bound(|| Ok(((), "here".to_string()))).expect("bound");
        assert_eq!(kept.address(), Some("here"));
        assert_eq!(kept.clone().address(), None);
        assert_eq!(format!("{kept:?}"), r#"Kept { address: Some("here") }"#);
    }

    #[test]
    fn a_receive_takes_what_the_open_bound_and_binds_nothing() {
        // Until 2026-11-16 the first receive bound the listener, after the
        // node had started (open problem 71).
        let kept: Kept<TcpListener> = Kept::new();
        let unbound = kept.listener().expect_err("nothing bound");
        assert!(!unbound.retryable, "{unbound}");
        assert!(unbound.message.contains("listening"), "{unbound}");
        let bound = kept
            .bound(|| socket::bind_tcp("127.0.0.1:0"))
            .expect("bound");
        let taken = kept.listener().expect("the one bound");
        assert!(std::ptr::eq(bound, taken));
    }
}
