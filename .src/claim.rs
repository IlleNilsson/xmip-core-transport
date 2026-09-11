//! One holder of a collidable artefact at a time — claimed at the endpoint.
//!
//! ADR-0024. This replaces exclusiveness, which held a lease inside Xmip and
//! could never say where a cluster-wide one would live.

use std::fmt;

use crate::error::Result;

/// One discrete claimable thing, addressed in its protocol's own terms.
///
/// `sftp://partner.example/out/order-1.edi`, an S3 key, a blob path, a message
/// uid in a mailbox.
///
/// **Not `xcore::ArtifactId`**, and the near-collision is worth the
/// sentence: an Xmip Artifact is a *configured object* — a Receive Location, a
/// Send Port — and this is a thing sitting at the far end of one. ADR-0017
/// spelled it `ArtefactId`, one letter from the other, which is how the two
/// would have been confused in a signature at some point.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Artefact(String);

impl Artefact {
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Artefact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Proof that the endpoint granted the claim.
///
/// Carries whatever the protocol handed back — a blob lease id, an `ETag`, a
/// handle — because releasing usually needs it and only the transport that
/// obtained it knows what it means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claimed {
    pub artefact: Artefact,
    pub token: String,
}

impl Claimed {
    #[must_use]
    pub fn new(artefact: Artefact, token: impl Into<String>) -> Self {
        Self {
            artefact,
            token: token.into(),
        }
    }
}

/// The endpoint's own claim, implemented by the transport that speaks the
/// protocol.
///
/// **The endpoint is one thing however many nodes are asking.** A claim taken
/// there is cluster-wide without a lease, a store, or anything for Xmip to keep
/// consistent across nodes, because the shared write path is the partner's
/// storage rather than Xmip's:
///
/// | family | native claim |
/// | --- | --- |
/// | local file, SMB | share-mode open, mandatory on Windows, advisory on Unix |
/// | Azure Blob | a renewable blob lease |
/// | S3 | `PUT` with `If-None-Match: *` on a claim key |
/// | Google Cloud Storage | a generation precondition |
/// | POP3 | the session locks the maildrop |
/// | SQL | `SELECT … FOR UPDATE SKIP LOCKED` |
///
/// It also answers what a lease could not: whether something **outside Xmip**
/// holds the artefact. A file another process has open includes a producer
/// still writing it.
///
/// ADR-0024 clause 4: **the artefact, not the location.** Two nodes may poll one
/// directory at the same time and take different files.
pub trait ResourceClaim: Send + Sync {
    /// Whether anything is using it, inside Xmip or outside.
    ///
    /// # Errors
    ///
    /// Where the endpoint could not be asked. Unreachable is not available.
    fn is_available(&self, artefact: &Artefact) -> Result<bool>;

    /// Take it. Atomic, or it is not a claim.
    ///
    /// # Errors
    ///
    /// Where somebody else holds it, or the endpoint could not be reached.
    fn claim(&self, artefact: &Artefact) -> Result<Claimed>;

    /// # Errors
    ///
    /// Where the endpoint refused. A claim that cannot be released is not a
    /// leak — it expires on the endpoint's own terms.
    fn release(&self, claimed: Claimed) -> Result<()>;
}

/// A protocol with artefacts and no locking. ADR-0024 clause 5.
///
/// FTP, SFTP and IMAP. Neither protocol has locking, and they are **not** made
/// safe by renaming the artefact at claim time — that puts a second mechanism
/// in the one place nothing is supposed to move, to defend against a non-Xmip
/// client polling the same directory, which no mechanism defends against
/// anyway.
///
/// What these transports need instead is a stability check: an artefact whose
/// size and timestamp are unchanged across two consecutive listings, or a
/// producer that writes to a temporary name and renames on completion.
///
/// Saying this once in a type beats every such transport writing the same three
/// stubs and one of them quietly returning something else.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoNativeClaim;

impl ResourceClaim for NoNativeClaim {
    fn is_available(&self, _artefact: &Artefact) -> Result<bool> {
        Ok(true)
    }

    fn claim(&self, artefact: &Artefact) -> Result<Claimed> {
        Ok(Claimed::new(artefact.clone(), String::new()))
    }

    fn release(&self, _claimed: Claimed) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_artefact_is_addressed_in_its_own_protocols_terms() {
        let artefact = Artefact::new("s3://bucket/in/order-1.edi");

        assert_eq!(artefact.address(), "s3://bucket/in/order-1.edi");
        assert_eq!(artefact.to_string(), "s3://bucket/in/order-1.edi");
    }

    #[test]
    fn a_protocol_with_artefacts_and_no_locking_says_so() {
        let ftp = NoNativeClaim;
        let artefact = Artefact::new("sftp://partner.example/out/order-1.edi");

        assert!(ftp.is_available(&artefact).expect("asked"));

        let claimed = ftp.claim(&artefact).expect("claimed");

        assert_eq!(claimed.artefact, artefact);
        assert!(
            claimed.token.is_empty(),
            "the endpoint granted nothing, so there is no token to carry"
        );
    }
}
