//! The endpoint's claim of a technology of the file family, written once
//! for all of them: a transport that reaches its location as a
//! [`ClaimSite`] and keeps its [`Claiming`] answers [`ResourceClaim`] for
//! one artifact as its receive claims every one (ADR-0024, amendment
//! 2026-10-13).

use crate::error::{Result, TransportError};

use super::claimant::may_arrive;
use super::claiming::Claiming;
use super::site::ClaimSite;
use super::{Artifact, Claimed, ResourceClaim};

/// The location as an act reaches it: a directory at hand, a control
/// connection or a channel kept for the server.
pub type Site<'a, S> = &'a dyn ClaimSite<Stamp = S>;

/// A transport whose artifacts are claimed at a location of the file
/// family.
pub trait Sited: Send + Sync {
    /// What says an artifact is unchanged there.
    type Stamp: Clone + Eq + Send + 'static;

    /// The Location's claims.
    fn claiming(&self) -> &Claiming<Self::Stamp>;

    /// Run `act` on the location — asked again on a new connection where a
    /// kept one failed.
    ///
    /// # Errors
    /// Where the location could not be reached, or as `act`.
    fn with_site<T>(&self, act: &mut dyn FnMut(Site<'_, Self::Stamp>) -> Result<T>) -> Result<T>;
}

/// An artifact's address, in whatever terms — a path, a URI — read to the
/// name it lies under in the location.
fn name_of(artifact: &Artifact) -> &str {
    artifact
        .address()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
}

/// The claim a receive takes, for a caller holding one artifact by its
/// address; the token is its claimed name.
impl<T: Sited> ResourceClaim for T {
    fn is_available(&self, artifact: &Artifact) -> Result<bool> {
        let name = name_of(artifact);
        if !may_arrive(name) {
            return Ok(false);
        }
        self.with_site(&mut |site| Ok(site.listed()?.iter().any(|(listed, _)| listed == name)))
    }

    fn claim(&self, artifact: &Artifact) -> Result<Claimed> {
        let name = name_of(artifact);
        let claimant = self.claiming().claimant();
        self.with_site(&mut |site| claimant.claim(site, name, || Ok(true)))?
            .map(|claimed| Claimed::new(artifact.clone(), claimed))
            .ok_or_else(|| TransportError::retryable(format!("{artifact} is held, or gone")))
    }

    fn release(&self, claimed: Claimed) -> Result<()> {
        let claiming = self.claiming();
        self.with_site(&mut |site| claiming.give_back(site, &claimed.token, false))
    }
}
