//! One protocol, both directions.

use crate::arrivals::Arrivals;
use crate::arrived::Arrived;
use crate::claim::ResourceClaim;
use crate::direction::Directions;
use crate::error::Result;
use crate::outgoing::Outgoing;
use crate::sending::Sending;
use crate::waiting::{Notice, Waiting};

/// What every protocol implements.
///
/// Nothing here knows which protocols exist. That is what makes lifting one
/// out into `xmip-core-transport-<name>` a move rather than a rewrite.
pub trait Transport {
    /// The standard token, as it appears in a repository name.
    fn name(&self) -> &'static str;

    /// Which directions this implementation actually supports.
    fn directions(&self) -> Directions;

    /// Take at most `most` of what has arrived — one at least, as the
    /// runtime has room for that many and waits for room before it asks.
    /// An empty vector when nothing has. **What it does not take stays at
    /// its far end**: a file left unclaimed in its directory, a message left
    /// on its broker, a row left in its table, for a later receive. A
    /// protocol that hands over one arrival a receive takes one whatever
    /// `most` says; one whose far end hands over what it holds at once and
    /// cannot keep the rest — a connection's messages taken at most once —
    /// takes them all, and its README says so.
    ///
    /// **Nothing is consumed here.** Each [`Arrived`] carries its body as a
    /// reader and its far end's acknowledgement; the far end keeps what
    /// arrived — the file in its directory, the message leased, the caller
    /// waiting for its answer — until the runtime gives the acknowledgement
    /// its verdict after the whole receive cycle (runtime-model section 5).
    /// Where the technology's arrivals are [`Arrivals::Ordered`], the runtime
    /// gives every arrival of one receive its verdict, in the order they
    /// were handed back, before it receives again, and the technology may
    /// rely on that; where they are [`Arrivals::Unordered`], it may be
    /// asked again before earlier arrivals are told. One dropped without a
    /// verdict consumes nothing. A protocol that cannot defer says so with
    /// [`crate::Acknowledgement::at_most_once`] and in its README.
    ///
    /// # Errors
    ///
    /// Where the endpoint could not be read. **Nothing there is not an error** —
    /// a drop directory that does not exist yet returns an empty vector.
    fn receive_up_to(&self, most: usize) -> Result<Vec<Arrived>>;

    /// Take whatever has arrived, as many as the far end hands over:
    /// [`Transport::receive_up_to`] without a bound. What a test or a tool
    /// asks; the runtime asks with its room.
    ///
    /// # Errors
    ///
    /// As [`Transport::receive_up_to`].
    fn receive(&self) -> Result<Vec<Arrived>> {
        self.receive_up_to(usize::MAX)
    }

    /// Whether this technology's arrivals are told in order, before the
    /// next receive, or each on its own, and why. No default: every
    /// technology says, in its own words.
    fn arrivals(&self) -> Arrivals;

    /// Whether this technology's receive waits for what arrives, or looks
    /// and returns at once — and why. No default: every technology says.
    /// After a polled receive that found nothing, the runtime waits its
    /// poll interval before it asks again; a waiting one it asks at once.
    fn waiting(&self) -> Waiting;

    /// Where this technology's far end tells of a change, where it is
    /// polled and can: the runtime waits for that word between receives,
    /// rather than its poll interval (`crate::waiting`). `None` where
    /// nothing tells.
    fn notice(&self) -> Option<Notice> {
        None
    }

    /// Whether this technology sends a body as it reads it, or gathers it
    /// whole first because its protocol needs it — and why. No default:
    /// every technology says. The runtime's byte budget for sends counts
    /// what a send holds by it.
    fn sending(&self) -> Sending;

    /// Deliver `outgoing` to a target expressed in this protocol's own
    /// terms: its body read from the Ledger a chunk at a time, written on
    /// as it is read ([`Outgoing::write_to`]) or gathered whole where the
    /// protocol needs it ([`Outgoing::gather`]), as [`Transport::sending`]
    /// says; and its deduplication key ([`Outgoing::key`]), the Journey's
    /// identifier, the same on every attempt of one Journey and different
    /// for every other (runtime-model section 15, *Delivery semantics*). A
    /// technology whose protocol carries a native identifier the far end
    /// deduplicates by — an AMQP `message-id`, a Kafka record key, an HTTP
    /// `Idempotency-Key`, a `JetStream` `Nats-Msg-Id` — puts the key there,
    /// and its README says so; a send repeated after a lost answer is then
    /// delivered once. Every other sends as it always does, at least once.
    ///
    /// # Errors
    ///
    /// Where the target is not addressable in this protocol, the endpoint
    /// refused or could not be reached, or the body could not be read or
    /// was not of its length.
    fn send_from(&self, target: &str, outgoing: Outgoing<'_>) -> Result<()>;

    /// Deliver bytes held in memory, under no key: [`Transport::send_from`]
    /// over them. What a test or a tool sends; the runtime sends from the
    /// Ledger.
    ///
    /// # Errors
    ///
    /// As [`Transport::send_from`].
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        self.send_from(target, Outgoing::held(bytes))
    }

    /// Why this technology's send keeps what its far end sees back until
    /// [`Outgoing::kept`] returns — a file written beside its target and
    /// renamed into place only then — or `None`, where the far end sees a
    /// send as it is made. The runtime hands a technology that holds back a
    /// Message whose Publication is still being synced to the Ledger, so
    /// the send's own work and the sync overlap; every other it hands the
    /// Message only once it is durable (ADR-0018, amendment 2026-10-20).
    fn holds_back(&self) -> Option<&'static str> {
        None
    }

    /// How this protocol claims a discrete artifact, where it can. ADR-0024.
    ///
    /// Three answers, and they are different:
    ///
    /// - `None` — no artifact to claim. A listening socket, a broker topic.
    /// - `Some(&NoNativeClaim)` — artifacts, and no claim built yet. IMAP.
    /// - `Some(&…)` — the protocol's own atomic claim; the file family's
    ///   turn and rename (`claim::Sited`): file, FTP, FTPS, SFTP.
    fn claims(&self) -> Option<&dyn ResourceClaim> {
        None
    }
}
