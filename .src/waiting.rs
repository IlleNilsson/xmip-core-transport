//! Whether a transport's receive waits for what arrives, or looks and
//! returns at once.
//!
//! The runtime asks a Receive Location's transport again as soon as a
//! receive returns. Where the receive itself waits — a listener's accept, a
//! socket's read, a broker delivering until it is quiet, a long poll — that
//! wait paces the Location, and an arrival is taken the moment it lands.
//! Where the receive looks and returns at once — a directory listed, a
//! table queried, a register read, an object store listed — asking again
//! at once after finding nothing spins: the processor and the far end
//! worked for no traffic. So a receive that found nothing on a polled
//! technology is followed by the node's poll interval (`[tuning]
//! receive_poll` in `xmip-core-runtime`), a wait a stop ends at once.
//!
//! **Every transport says which, and why** ([`crate::Transport::waiting`]),
//! as it says how its arrivals are told ([`crate::Arrivals`]): there is no
//! default that guesses.
//!
//! **A polled technology whose far end tells of a change says where**
//! ([`crate::Transport::notice`], [`Notice`]): the runtime then waits for
//! that word rather than its poll interval, and lists again the moment it
//! comes, so an arrival is taken as it lands; the poll becomes a recovery
//! sweep at its own cadence (`[tuning] receive_sweep`), for what lay there
//! before the watch began or a word that was missed. The file transport's
//! drop directory is one, told by the file system's change notification
//! (ADR-0024, amendment 2026-10-20); a database's `LISTEN` would be another.

/// How a transport's receive meets a far end with nothing for it, and the
/// reason in words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waiting {
    /// The receive waits until something arrives or its own wait runs
    /// out: the runtime asks again at once.
    Waits(&'static str),
    /// The receive looks and returns at once: after one that found
    /// nothing, the runtime waits its poll interval before it asks again.
    Polled(&'static str),
}

impl Waiting {
    /// Whether the receive returns at once, so a receive that found
    /// nothing is followed by the poll interval.
    #[must_use]
    pub const fn polled(self) -> bool {
        matches!(self, Self::Polled(_))
    }

    /// Why, in words an operator reads.
    #[must_use]
    pub const fn because(self) -> &'static str {
        match self {
            Self::Waits(because) | Self::Polled(because) => because,
        }
    }
}

/// Where a polled technology's far end tells of a change, for the runtime
/// to wait on between its receives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Notice {
    /// A directory of this computer's file system, whose change
    /// notification tells of a file created in it, renamed into it, written
    /// or removed.
    Directory(std::path::PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_says_whether_a_receive_is_polled_and_why() {
        let listed = Waiting::Polled("a directory is listed and the receive returns");
        let accepted = Waiting::Waits("the accept waits for a connection");
        assert!(listed.polled());
        assert!(!accepted.polled());
        assert_eq!(
            listed.because(),
            "a directory is listed and the receive returns"
        );
        assert_eq!(accepted.because(), "the accept waits for a connection");
    }
}
