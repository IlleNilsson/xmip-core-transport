//! One protocol, both directions.

use crate::arrivals::Arrivals;
use crate::arrived::Arrived;
use crate::claim::ResourceClaim;
use crate::direction::Directions;
use crate::error::Result;

/// What every protocol implements.
///
/// Seven methods, and nothing here knows which protocols exist. That is what
/// makes lifting one out into `xmip-core-transport-<name>` a move rather than a
/// rewrite.
pub trait Transport {
    /// The standard token, as it appears in a repository name.
    fn name(&self) -> &'static str;

    /// Which directions this implementation actually supports.
    fn directions(&self) -> Directions;

    /// Take whatever has arrived. An empty vector when nothing has.
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
    /// asked again before earlier arrivals are told. One dropped without a verdict consumes nothing. A
    /// protocol that cannot defer says so with
    /// [`crate::Acknowledgement::at_most_once`] and in its README.
    ///
    /// # Errors
    ///
    /// Where the endpoint could not be read. **Nothing there is not an error** —
    /// a drop directory that does not exist yet returns an empty vector.
    fn receive(&self) -> Result<Vec<Arrived>>;

    /// Whether this technology's arrivals are told in order, before the
    /// next receive, or each on its own, and why. No default: every
    /// technology says, in its own words.
    fn arrivals(&self) -> Arrivals;

    /// Deliver bytes to a target expressed in this protocol's own terms.
    ///
    /// # Errors
    ///
    /// Where the target is not addressable in this protocol, or the endpoint
    /// refused or could not be reached.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()>;

    /// Deliver bytes as [`Transport::send`] does, under `key`: the
    /// delivery's deduplication key, the Journey's identifier, the same on
    /// every attempt of one Journey and different for every other
    /// (runtime-model section 15, *Delivery semantics*). A technology whose
    /// protocol carries a native identifier the far end deduplicates by —
    /// an AMQP `message-id`, a Kafka record key, an HTTP `Idempotency-Key`,
    /// a `JetStream` `Nats-Msg-Id` — puts `key` there, and its README says
    /// so; a send repeated after a lost answer is then delivered once.
    /// Every other technology sends as it always does, at least once: the
    /// default ignores `key`.
    ///
    /// # Errors
    ///
    /// As [`Transport::send`].
    fn send_keyed(&self, target: &str, bytes: &[u8], key: &str) -> Result<()> {
        let _ = key;
        self.send(target, bytes)
    }

    /// How this protocol claims a discrete artefact, where it can. ADR-0024.
    ///
    /// Three answers, and they are different:
    ///
    /// - `None` — no artefact to claim. A listening socket, a broker topic.
    /// - `Some(&NoNativeClaim)` — artefacts, and no locking. FTP, SFTP, IMAP.
    /// - `Some(&…)` — the protocol's own atomic claim.
    fn claims(&self) -> Option<&dyn ResourceClaim> {
        None
    }
}
