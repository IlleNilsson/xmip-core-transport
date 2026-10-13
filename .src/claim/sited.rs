//! The endpoint's claim of a technology of the file family, written once
//! for all of them: a transport that reaches its location as a
//! [`ClaimSite`] and keeps its [`Claiming`] answers [`ResourceClaim`] for
//! one artefact as its receive claims every one (ADR-0024, amendment
//! 2026-10-13).

use crate::error::{Result, TransportError};

use super::claimant::is_claimed;
use super::claiming::Claiming;
use super::site::ClaimSite;
use super::{Artefact, Claimed, ResourceClaim};

/// The location as an act reaches it: a directory at hand, a control
/// connection or a channel kept for the server.
pub type Site<'a, S> = &'a dyn ClaimSite<Stamp = S>;

/// A transport whose artefacts are claimed at a location of the file
/// family.
pub trait Sited: Send + Sync {
    /// What says an artefact is unchanged there.
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

/// An artefact's address, in whatever terms — a path, a URI — read to the
/// name it lies under in the location.
fn name_of(artefact: &Artefact) -> &str {
    artefact
        .address()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
}

/// The claim a receive takes, for a caller holding one artefact by its
/// address; the token is its claimed name.
impl<T: Sited> ResourceClaim for T {
    fn is_available(&self, artefact: &Artefact) -> Result<bool> {
        let name = name_of(artefact);
        if is_claimed(name) {
            return Ok(false);
        }
        self.with_site(&mut |site| Ok(site.listed()?.iter().any(|(listed, _)| listed == name)))
    }

    fn claim(&self, artefact: &Artefact) -> Result<Claimed> {
        let name = name_of(artefact);
        let claimant = self.claiming().claimant();
        self.with_site(&mut |site| claimant.claim(site, name, || Ok(true)))?
            .map(|claimed| Claimed::new(artefact.clone(), claimed))
            .ok_or_else(|| TransportError::retryable(format!("{artefact} is held, or gone")))
    }

    fn release(&self, claimed: Claimed) -> Result<()> {
        let claiming = self.claiming();
        self.with_site(&mut |site| claiming.give_back(site, &claimed.token, false))
    }
}
