//! The callers a Receive Location turned away at their own connection,
//! kept until the runtime asks ([`crate::Transport::turned_away`]), which
//! audits each as a refusal of that caller (ADR-0013 clause 1) while the
//! Location goes on serving the others.
//!
//! A failure that belongs to one connection — its own I/O, a head or frame
//! past its bound, bytes that are not the protocol, a read past the
//! Location's timeout, a connection that could not be readied — ends that
//! connection and nothing more ([`crate::serving`]). Until 2026-11-13 it was the
//! receive's failure, and one asking again would not mend stopped the
//! Location for every other caller: one broken or hostile caller took it down.

use std::net::SocketAddr;

use crate::error::TransportError;

/// The most callers kept between two asks; any more are counted only, so a
/// flood of broken callers holds bounded memory however long one receive
/// waits.
pub const MOST_HELD: usize = 64;

/// One caller turned away: where it called from, and what its connection
/// did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnedAway {
    pub peer: SocketAddr,
    pub why: TransportError,
}

/// The callers turned away since the runtime last asked: the first
/// [`MOST_HELD`] of them, and how many came beyond those.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turned {
    pub callers: Vec<TurnedAway>,
    pub beyond: usize,
}

impl Turned {
    /// Nobody turned away.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            callers: Vec::new(),
            beyond: 0,
        }
    }

    /// `peer` turned away, for `why`: kept, or counted past [`MOST_HELD`].
    pub fn away(&mut self, peer: SocketAddr, why: TransportError) {
        if self.callers.len() < MOST_HELD {
            self.callers.push(TurnedAway { peer, why });
        } else {
            self.beyond += 1;
        }
    }

    /// Whether nobody was turned away.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.callers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_past_the_most_held_are_counted_not_kept() {
        let peer: SocketAddr = "192.0.2.10:4711".parse().expect("an address");
        let mut turned = Turned::default();
        assert!(turned.is_empty());
        for _ in 0..MOST_HELD + 3 {
            turned.away(peer, TransportError::permanent("a head past its bound"));
        }
        assert_eq!((turned.callers.len(), turned.beyond), (MOST_HELD, 3));
        assert_eq!(turned.callers[0].peer, peer);
        assert!(!turned.is_empty());
    }
}
