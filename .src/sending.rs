//! Whether a transport sends a body as it reads it, or gathers it whole
//! first.
//!
//! The runtime hands every send its Message a chunk at a time from the
//! Ledger ([`crate::Outgoing`]). A technology that writes the body on as it
//! comes — a file, a byte stream, a transfer with its length ahead of it —
//! holds a piece of it at a time, whatever its size. One whose protocol
//! needs the whole body before its first byte goes — a datagram, a frame,
//! a record, a message with its length counted in a form only the whole
//! gives, a signature or a digest over all of it — holds all of it while it
//! sends. The runtime's byte budget for sends counts what each holds by
//! this (`xmip-core-runtime`'s `send_step`), so a gathered send of a large
//! Message waits for room rather than growing the node.
//!
//! **Every transport says which, and why** ([`crate::Transport::sending`]),
//! as it says how its arrivals are told ([`crate::Arrivals`]) and whether
//! its receive waits ([`crate::Waiting`]): there is no default that
//! guesses.

/// How a transport sends a body, and the reason in words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sending {
    /// Written on as it is read: the send holds a piece of the body at a
    /// time ([`crate::Outgoing::write_to`]).
    Streamed(&'static str),
    /// Gathered whole before it is sent, as its protocol needs: the send
    /// holds the whole body ([`crate::Outgoing::gather`]).
    Gathered(&'static str),
}

impl Sending {
    /// Whether the send holds the whole body while it sends.
    #[must_use]
    pub const fn gathered(self) -> bool {
        matches!(self, Self::Gathered(_))
    }

    /// Why, in words an operator reads.
    #[must_use]
    pub const fn because(self) -> &'static str {
        match self {
            Self::Streamed(because) | Self::Gathered(because) => because,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sending_says_whether_the_whole_body_is_held_and_why() {
        let written = Sending::Streamed("the file is written as the body is read");
        let datagram = Sending::Gathered("one datagram carries the whole body");
        assert!(!written.gathered());
        assert!(datagram.gathered());
        assert_eq!(datagram.because(), "one datagram carries the whole body");
    }
}
