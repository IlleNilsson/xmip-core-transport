//! One protocol, both directions.

use crate::arrived::Arrived;
use crate::claim::ResourceClaim;
use crate::direction::Directions;
use crate::error::Result;

/// What every protocol implements.
///
/// Five methods, and nothing here knows which protocols exist. That is what
/// makes lifting one out into `xmip-core-transport-<name>` a move rather than a
/// rewrite.
pub trait Transport {
    /// The standard token, as it appears in a repository name.
    fn name(&self) -> &'static str;

    /// Which directions this implementation actually supports.
    fn directions(&self) -> Directions;

    /// Take whatever has arrived. An empty vector when nothing has.
    ///
    /// # Errors
    ///
    /// Where the endpoint could not be read. **Nothing there is not an error** —
    /// a drop directory that does not exist yet returns an empty vector.
    fn receive(&self) -> Result<Vec<Arrived>>;

    /// Deliver bytes to a target expressed in this protocol's own terms.
    ///
    /// # Errors
    ///
    /// Where the target is not addressable in this protocol, or the endpoint
    /// refused or could not be reached.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()>;

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
