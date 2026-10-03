//! Whether a transport's arrivals are told in order, or each on its own.
//!
//! The runtime carries what a receive hands back on a Receive Location's
//! pool of threads. Where the far end's position matters — a cursor that
//! moves only contiguously, a queue settled in order, a directory or a
//! table polled again from the top — every arrival of one receive is told
//! in order and before the transport is asked again, or a second receive
//! would read what the first has not settled. Where it does not — each
//! connection, request or datagram its own, a lease or a lock keeping what
//! is in its cycle from being handed out twice — the transport is asked
//! again at once and the arrivals are carried side by side, up to the
//! pool's bound: two senders never wait on each other, and their Ledger
//! writes share a disk sync.
//!
//! **Every transport says which, and why** ([`crate::Transport::arrivals`]):
//! there is no default that guesses.

/// How a transport's arrivals are told, and the reason in words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrivals {
    /// Told in the order they came, every one before the next receive: the
    /// far end's position depends on it.
    Ordered(&'static str),
    /// Each its own: the transport may be asked again before earlier
    /// arrivals are told, and they are carried side by side.
    Unordered(&'static str),
}

impl Arrivals {
    /// Whether every arrival must be told, in order, before the next
    /// receive.
    #[must_use]
    pub const fn ordered(self) -> bool {
        matches!(self, Self::Ordered(_))
    }

    /// Why, in words an operator reads.
    #[must_use]
    pub const fn because(self) -> &'static str {
        match self {
            Self::Ordered(because) | Self::Unordered(because) => because,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrivals_say_whether_they_are_ordered_and_why() {
        let cursor = Arrivals::Ordered("a cursor moves only contiguously");
        let connections = Arrivals::Unordered("each connection is its own");
        assert!(cursor.ordered());
        assert!(!connections.ordered());
        assert_eq!(cursor.because(), "a cursor moves only contiguously");
        assert_eq!(connections.because(), "each connection is its own");
    }
}
