//! One holder of a collidable artifact at a time — claimed at the endpoint.
//!
//! ADR-0024. This replaced ADR-0017's exclusiveness lease, which lived
//! inside Xmip and could never say where a cluster-wide one would live.
//! Exclusiveness itself is Xmip's correlation sets (the owner, 2026-10-27;
//! ADR-0024 and ADR-0066, amendments 2026-10-27): this claim is its part at
//! the endpoint, one holder of an artifact at a time.
//!
//! The file family's claim — a turn, then a rename to a name recording the
//! claimant, the return, the recovery of a starting node — is written once
//! here (amendments 2026-09-26 and 2026-10-13), over the few operations
//! each technology of the family implements ([`ClaimSite`]):
//!
//! ```text
//!   site.rs       the operations at the far end: list, take a turn, rename,
//!                 restore, delete
//!   claimant.rs   who claims, the names a claim writes, the sequence, and
//!                 the recovery of what a starting node's name still holds
//!   claiming.rs   one Location's claims: the stability check, the verdict,
//!                 and what waits to go back
//!   hold.rs       a claimed artifact's acknowledgement, returned if let go
//!   sited.rs      the endpoint's claim of one artifact, for any technology
//!                 of the family
//!   unfinished.rs the name a sent file is written under beside its target,
//!                 carrying the node, and its removal as that node starts
//!                 (amendment 2026-10-19)
//! ```

mod claimant;
mod claiming;
mod hold;
#[cfg(test)]
mod memory;
mod site;
mod sited;
mod unfinished;

use std::fmt;

use crate::error::Result;

pub use claimant::{
    Claim, Claimant, MARK, TURN, claim_of, is_claimed, is_turn, may_arrive, turn_of,
};
pub use claiming::{Claiming, Held, Taken};
pub use site::{ClaimSite, Listed, Restore};
pub use sited::{Site, Sited};
pub use unfinished::{SENDING, Unfinished, is_unfinished, sender_of};

/// One discrete claimable thing, addressed in its protocol's own terms.
///
/// `sftp://party.example/out/order-1.edi`, an S3 key, a blob path, a message
/// uid in a mailbox.
///
/// **Not `xcore::ArtifactId`**, and the near-collision is worth the
/// sentence: an Xmip Artifact is a *configured object* — a Receive Location, a
/// Send Port — and this is a thing sitting at the far end of one. ADR-0017
/// spelled it `ArtifactId`, one letter from the other, which is how the two
/// would have been confused in a signature at some point.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Artifact(String);

impl Artifact {
    #[must_use]
    pub fn new(address: impl Into<String>) -> Self {
        Self(address.into())
    }

    #[must_use]
    pub fn address(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Artifact {
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
    pub artifact: Artifact,
    pub token: String,
}

impl Claimed {
    #[must_use]
    pub fn new(artifact: Artifact, token: impl Into<String>) -> Self {
        Self {
            artifact,
            token: token.into(),
        }
    }
}

/// The endpoint's own claim, implemented by the transport that speaks the
/// protocol.
///
/// **The endpoint is one thing however many nodes are asking.** A claim taken
/// there is cluster-wide without a lease, a store, or anything for Xmip to keep
/// consistent across nodes, because the shared write path is the Party's
/// storage rather than Xmip's:
///
/// | family | native claim |
/// | --- | --- |
/// | file, FTP, FTPS, SFTP | a turn, then a rename to a name recording the node and the time |
/// | Azure Blob | a renewable blob lease |
/// | S3 | `PUT` with `If-None-Match: *` on a claim key |
/// | Google Cloud Storage | a generation precondition |
/// | POP3 | the session locks the maildrop |
/// | SQL | `SELECT … FOR UPDATE SKIP LOCKED` |
///
/// It also answers what a lease could not: whether something **outside Xmip**
/// holds the artifact. A file another process has open includes a producer
/// still writing it; where the claim cannot see a writer, as a rename
/// cannot, the transport takes only what its stability check says is
/// finished (ADR-0024 clause 5 and its amendment of 2026-09-26).
///
/// ADR-0024 clause 4: **the artifact, not the location.** Two nodes may poll one
/// directory at the same time and take different files.
pub trait ResourceClaim: Send + Sync {
    /// Whether anything is using it, inside Xmip or outside.
    ///
    /// # Errors
    ///
    /// Where the endpoint could not be asked. Unreachable is not available.
    fn is_available(&self, artifact: &Artifact) -> Result<bool>;

    /// Take it. Atomic, or it is not a claim.
    ///
    /// # Errors
    ///
    /// Where somebody else holds it, or the endpoint could not be reached.
    fn claim(&self, artifact: &Artifact) -> Result<Claimed>;

    /// # Errors
    ///
    /// Where the endpoint refused. A claim that cannot be released is not a
    /// leak — it expires on the endpoint's own terms.
    fn release(&self, claimed: Claimed) -> Result<()>;
}

/// A protocol with artifacts whose claim is not built yet. ADR-0024 clause 5.
///
/// IMAP, which offers no exclusive step at all, and the object stores,
/// databases and shares whose own exclusive step is still to be taken
/// (ADR-0024, amendment 2026-10-13, which records each one's answer). The
/// file family — file, FTP, FTPS, SFTP — claims through [`Claiming`] since
/// that amendment, and answers its own claim.
///
/// Saying this once in a type beats every such transport writing the same three
/// stubs and one of them quietly returning something else.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoNativeClaim;

impl ResourceClaim for NoNativeClaim {
    fn is_available(&self, _artifact: &Artifact) -> Result<bool> {
        Ok(true)
    }

    fn claim(&self, artifact: &Artifact) -> Result<Claimed> {
        Ok(Claimed::new(artifact.clone(), String::new()))
    }

    fn release(&self, _claimed: Claimed) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::memory::Memory;
    use super::*;
    use crate::acknowledgement::{Refusal, Verdict};
    use crate::node::NodeLocation;

    fn claiming(place: usize) -> Claiming<u64> {
        let node = NodeLocation::new(format!("process-{}/place-{place}", std::process::id()));
        Claiming::new(Claimant::of(&node))
    }

    /// What two listings take: the first finds, the second takes what the
    /// first found unchanged.
    fn settled(claims: &Claiming<u64>, site: &Memory) -> Vec<Taken> {
        assert!(
            claims.take(site, usize::MAX).expect("listed").is_empty(),
            "found first"
        );
        claims.take(site, usize::MAX).expect("listed again")
    }

    #[test]
    fn two_claimants_racing_take_every_artifact_exactly_once() {
        const FILES: usize = 400;
        let site = Memory::default();
        for file in 0..FILES {
            site.drop_file(&format!("order-{file}.edi"), file.to_string().as_bytes());
        }
        let claimants = [claiming(0), claiming(1)];
        let took: Vec<Vec<Vec<u8>>> = std::thread::scope(|scope| {
            let racing: Vec<_> = claimants
                .iter()
                .map(|claims| {
                    let site = site.clone();
                    scope.spawn(move || {
                        let mut took = Vec::new();
                        while !site.names().is_empty() {
                            for taken in claims.take(&site, usize::MAX).expect("taken") {
                                took.push(site.read(&taken.claimed).expect("claimed"));
                                claims
                                    .told(&site, &taken.claimed, Some(Verdict::Accepted))
                                    .expect("accepted");
                            }
                        }
                        took
                    })
                })
                .collect();
            racing
                .into_iter()
                .map(|one| one.join().expect("a claimant"))
                .collect()
        });
        let all: Vec<&Vec<u8>> = took.iter().flatten().collect();
        let once: std::collections::BTreeSet<&Vec<u8>> = all.iter().copied().collect();
        assert_eq!(all.len(), FILES, "every artifact taken, none twice");
        assert_eq!(once.len(), FILES);
    }

    #[test]
    fn a_turn_taken_leaves_the_artifact_and_a_refused_rename_drops_the_turn() {
        let site = Memory::default();
        site.drop_file("order.edi", b"ISA");
        let claims = claiming(0);
        assert!(
            site.take_turn(&turn_of("order.edi"), "another")
                .expect("taken")
        );
        assert!(settled(&claims, &site).is_empty(), "another holds its turn");
        site.drop_turn(&turn_of("order.edi"), "another")
            .expect("dropped");
        site.refuse_renaming("order.edi");
        assert!(
            claims.take(&site, usize::MAX).expect("listed").is_empty(),
            "refused"
        );
        assert_eq!(
            site.names().len(),
            1,
            "its turn is gone: {:?}",
            site.names()
        );
    }

    #[test]
    fn a_listing_claims_no_more_than_asked_and_leaves_the_rest_for_the_next() {
        let site = Memory::default();
        for file in 0..10 {
            site.drop_file(&format!("order-{file}.edi"), b"ISA");
        }
        let claims = claiming(0);
        assert!(
            claims.take(&site, 3).expect("found").is_empty(),
            "found first"
        );
        let mut took = 0;
        for _ in 0..4 {
            let taken = claims.take(&site, 3).expect("taken");
            assert!(taken.len() <= 3, "{} claimed at once", taken.len());
            took += taken.len();
            let claimed = site.names().iter().filter(|name| is_claimed(name)).count();
            assert_eq!(claimed, took, "the rest unclaimed: {:?}", site.names());
        }
        assert_eq!(took, 10, "each finished one taken by the next listing");
    }

    #[test]
    fn a_changed_artifact_waits_for_a_listing_that_finds_it_unchanged() {
        let site = Memory::default();
        let claims = claiming(0);
        site.drop_file("order.edi", b"ISA");
        assert_eq!(
            claims.take(&site, usize::MAX).expect("found"),
            [] as [Taken; 0]
        );
        site.drop_file("order.edi", b"ISA*GS");
        assert!(
            claims.take(&site, usize::MAX).expect("grown").is_empty(),
            "it changed"
        );
        let taken = claims.take(&site, usize::MAX).expect("unchanged");
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].dropped, "order.edi");
        assert!(taken[0].claimed.starts_with("order.edi.xmip-claim."));
    }

    #[test]
    fn a_refusal_returns_and_remembers_and_a_newer_file_holds_a_return() {
        let site = Memory::default();
        let claims = claiming(0);
        site.drop_file("order.edi", b"old");
        let taken = settled(&claims, &site).remove(0);
        site.drop_file("order.edi", b"new");
        let refused = Some(Verdict::Refused(Refusal::Unacceptable));
        let told = claims.told(&site, &taken.claimed, refused);
        assert!(told.is_err_and(|held| held.retryable), "held");
        assert!(claims.held()[0].why.contains("dropped since"));
        let newer = settled(&claims, &site).remove(0);
        claims
            .told(&site, &newer.claimed, Some(Verdict::Accepted))
            .expect("consumed");
        assert!(claims.held().is_empty(), "its name freed, it went back");
        assert_eq!(site.read("order.edi").expect("returned"), b"old");
        for _ in 0..3 {
            assert!(
                claims.take(&site, usize::MAX).expect("listed").is_empty(),
                "refused"
            );
        }
        assert_eq!(settled(&claiming(1), &site).len(), 1, "another finds it");
    }

    #[test]
    fn a_starting_claimant_returns_its_own_claims_and_turns_and_no_others() {
        let site = Memory::default();
        site.drop_file("mine.edi", b"1");
        assert_eq!(settled(&claiming(0), &site).len(), 1);
        site.drop_file("theirs.edi", b"2");
        assert_eq!(settled(&claiming(1), &site).len(), 1);
        // Both die holding their claims, and the first a turn as well.
        let me = claiming(0);
        assert!(
            site.take_turn(&turn_of("x.edi"), me.claimant().as_str())
                .expect("turn")
        );
        assert!(site.names().iter().all(|name| is_claimed(name)));

        me.recover(&site).expect("recovered");
        let after = site.names();
        assert!(
            after.contains("mine.edi"),
            "its own claim returned: {after:?}"
        );
        assert!(
            after
                .iter()
                .any(|name| name.starts_with("theirs.edi") && name.contains(MARK)),
            "another's claim is its own: {after:?}"
        );
        assert!(
            !after.contains(&turn_of("x.edi")),
            "its turn removed: {after:?}"
        );
    }

    struct Located(Memory, Claiming<u64>);

    impl Sited for Located {
        type Stamp = u64;
        fn claiming(&self) -> &Claiming<u64> {
            &self.1
        }
        fn with_site<T>(&self, act: &mut dyn FnMut(Site<'_, u64>) -> Result<T>) -> Result<T> {
            act(&self.0)
        }
    }

    #[test]
    fn one_artifact_is_claimed_by_its_address_and_released_back() {
        let located = Located(Memory::default(), claiming(0));
        located.0.drop_file("order.edi", b"ISA");
        let artifact = Artifact::new("sftp://party.example/out/order.edi");
        assert!(located.is_available(&artifact).expect("asked"));
        let claimed = located.claim(&artifact).expect("claimed");
        assert!(
            claimed.token.starts_with("order.edi.xmip-claim."),
            "{claimed:?}"
        );
        assert!(!located.is_available(&artifact).expect("asked"), "held");
        assert!(located.claim(&artifact).is_err_and(|held| held.retryable));
        located.release(claimed).expect("released");
        assert!(located.is_available(&artifact).expect("asked again"));
    }

    #[test]
    fn a_claimed_name_reads_back_and_a_turn_is_never_an_arrival() {
        let claim = claim_of("order.edi.xmip-claim.node%2F1.1790000000123456789");
        assert_eq!(
            claim,
            Some(Claim {
                dropped: "order.edi",
                claimant: "node%2F1"
            })
        );
        assert!(is_claimed("order.edi.xmip-claim"));
        assert!(!is_claimed("order.edi"));
        assert!(claim_of("order.edi.xmip-claim.node.later").is_none());
    }

    #[test]
    fn an_artifact_is_addressed_in_its_own_protocols_terms() {
        let artifact = Artifact::new("s3://bucket/in/order-1.edi");

        assert_eq!(artifact.address(), "s3://bucket/in/order-1.edi");
        assert_eq!(artifact.to_string(), "s3://bucket/in/order-1.edi");
    }

    #[test]
    fn a_protocol_with_artifacts_and_no_locking_says_so() {
        let imap = NoNativeClaim;
        let artifact = Artifact::new("imap://party.example/INBOX;UID=1");

        assert!(imap.is_available(&artifact).expect("asked"));

        let claimed = imap.claim(&artifact).expect("claimed");

        assert_eq!(claimed.artifact, artifact);
        assert!(
            claimed.token.is_empty(),
            "the endpoint granted nothing, so there is no token to carry"
        );
    }
}
