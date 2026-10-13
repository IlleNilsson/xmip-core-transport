//! One Receive Location's claims over a location of the file family: what
//! it takes, what it hands back, and what waits to go back.
//!
//! **A file is taken once it is finished, and by one claimant.** A listing
//! takes an artefact only where its stamp is what the listing before found
//! (ADR-0024 clause 5, the stability check, which the amendment of
//! 2026-10-13 keeps), and still is once its turn is held; it is then
//! claimed ([`Claimant::claim`]). What the listing did not take is
//! remembered for the next one, so the memory is never more than the far
//! end holds.
//!
//! **A file is consumed only on its verdict** ([`Claiming::told`]):
//! `Accepted` deletes the claimed artefact; `Refused` returns it under its
//! dropped name and remembers it as refused (`transport::Refused`), so it is
//! not received again while it lies so; `Failed`, or no verdict at all,
//! returns it, and a later listing finds it again.
//!
//! **A return never waits for a restart, and never replaces.** An artefact
//! is never returned over one dropped under its name since; it is then
//! held under its claimed name with why ([`Claiming::held`]) and goes back
//! as soon as the name is free — after this Location consumes the newer
//! one, or at the next listing. Until 2026-10-05 such a file stayed claimed
//! until its node started again.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::acknowledgement::Verdict;
use crate::error::{Result, TransportError};
use crate::refused::Refused;

use super::claimant::{Claimant, claim_of, is_claimed};
use super::site::{ClaimSite, Restore};

/// An artefact a listing claimed: the name it was dropped with, and the
/// name it lies under while claimed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Taken {
    pub dropped: String,
    pub claimed: String,
}

/// A claimed artefact that could not go back under its dropped name yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Held {
    /// Where it lies: its claimed name.
    pub claimed: String,
    /// Why it lies there and not under the name it was dropped with.
    pub why: String,
    /// Whether its verdict was a refusal: returned, it is remembered as
    /// refused, as it would have been had it gone back at once.
    pub refused: bool,
}

/// One Location's claims. Cloned, it is the same memory: a transport and
/// the acknowledgements it hands out share it.
pub struct Claiming<S> {
    inner: Arc<Inner<S>>,
}

struct Inner<S> {
    claimant: Claimant,
    /// Whether what the claimant's name still holds is yet to be returned,
    /// by the first listing that reaches the far end.
    recovering: AtomicBool,
    /// What the last listing found and did not take: the stability check's
    /// other half.
    listed: Mutex<HashMap<String, Option<S>>>,
    refused: Refused<String, S>,
    held: Mutex<BTreeMap<String, Held>>,
}

impl<S> Clone for Claiming<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S: Clone + Eq + Send + 'static> Claiming<S> {
    /// Claims made as `claimant`, nothing remembered yet.
    #[must_use]
    pub fn new(claimant: Claimant) -> Self {
        Self::built(claimant, false)
    }

    /// Claims made as `claimant`, a node starting: what its name still
    /// holds is returned by the first listing that reaches the far end
    /// ([`Self::recover`]) — a server out of reach as the node starts does
    /// not stop it, and a Send Location never connects for it.
    #[must_use]
    pub fn recovering(claimant: Claimant) -> Self {
        Self::built(claimant, true)
    }

    fn built(claimant: Claimant, recovering: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                claimant,
                recovering: AtomicBool::new(recovering),
                listed: Mutex::default(),
                refused: Refused::default(),
                held: Mutex::default(),
            }),
        }
    }

    /// Who claims.
    #[must_use]
    pub fn claimant(&self) -> &Claimant {
        &self.inner.claimant
    }

    /// What lies refused, as this Location remembers it.
    #[must_use]
    pub fn refused(&self) -> &Refused<String, S> {
        &self.inner.refused
    }

    /// Every artefact at `site` that is finished, not refused as it lies,
    /// and won: each claimed. A held artefact whose name was freed goes
    /// back first, and is listed with the rest.
    ///
    /// # Errors
    /// Where `site` could not be listed, or the far end failed before
    /// anything was claimed. What was claimed before a failure is handed
    /// back; the rest is left to the next listing.
    pub fn take<P: ClaimSite<Stamp = S> + ?Sized>(&self, site: &P) -> Result<Vec<Taken>> {
        if self.inner.recovering.swap(false, Ordering::SeqCst) && self.recover(site).is_err() {
            self.inner.recovering.store(true, Ordering::SeqCst);
        }
        self.settle(site);
        let found: Vec<_> = site
            .listed()?
            .into_iter()
            .filter(|(name, _)| !is_claimed(name))
            .collect();
        let found = self
            .inner
            .refused
            .sift(found, |(name, _)| name, |(_, stamp)| stamp.clone());
        let mut listed = lock(&self.inner.listed);
        let mut seen = HashMap::new();
        let mut taken = Vec::new();
        let mut broken = None;
        for (name, stamp) in found {
            // Unchanged since the last listing, and still so once its turn
            // is held: finished, and the artefact that was found.
            if broken.is_some() || listed.get(&name) != Some(&stamp) {
                seen.insert(name, stamp);
                continue;
            }
            let still = || site.stamp(&name).map(|now| now == stamp);
            match self.inner.claimant.claim(site, &name, still) {
                Ok(Some(claimed)) => taken.push(Taken {
                    dropped: name,
                    claimed,
                }),
                Ok(None) => {
                    seen.insert(name, stamp);
                }
                Err(failed) => {
                    seen.insert(name, stamp);
                    broken = Some(failed);
                }
            }
        }
        *listed = seen;
        match broken {
            Some(failed) if taken.is_empty() => Err(failed),
            _ => Ok(taken),
        }
    }

    /// The verdict on the artefact claimed as `claimed`, `None` where it was
    /// let go without one: deleted on `Accepted` where `consume` says,
    /// otherwise returned — and remembered as refused on `Refused`.
    ///
    /// # Errors
    /// Where the far end refused the delete, or the artefact could not go
    /// back now: why, and that it is held. Retryable then, since it goes
    /// back without anyone asking again.
    pub fn told<P: ClaimSite<Stamp = S> + ?Sized>(
        &self,
        site: &P,
        claimed: &str,
        verdict: Option<Verdict>,
        consume: bool,
    ) -> Result<()> {
        match verdict {
            Some(Verdict::Accepted) if consume => {
                site.delete(claimed)?;
                // Its dropped name is free now: one waiting for it goes back.
                self.settle(site);
                Ok(())
            }
            Some(Verdict::Refused(_)) => self.give_back(site, claimed, true),
            _ => self.give_back(site, claimed, false),
        }
    }

    /// The artefact claimed as `claimed` back under its dropped name,
    /// remembered as refused where `refused` says. Where it cannot go back
    /// now, it is held with why and tried again as soon as its name may be
    /// free.
    ///
    /// # Errors
    /// Where it was not a claimed name, or could not go back now.
    pub fn give_back<P: ClaimSite<Stamp = S> + ?Sized>(
        &self,
        site: &P,
        claimed: &str,
        refused: bool,
    ) -> Result<()> {
        let dropped = claim_of(claimed)
            .map(|claim| claim.dropped.to_string())
            .ok_or_else(|| TransportError::permanent(format!("{claimed} is not a claimed name")))?;
        let why = match site.restore(claimed, &dropped) {
            Ok(Restore::Back) => {
                if refused {
                    self.remember(site, dropped);
                }
                return Ok(());
            }
            Ok(Restore::Gone) => return Ok(()),
            Ok(Restore::Kept(why)) => why,
            Err(failed) => failed.message,
        };
        let why = held_because(&why, claimed);
        lock(&self.inner.held).insert(
            claimed.to_string(),
            Held {
                claimed: claimed.to_string(),
                why: why.clone(),
                refused,
            },
        );
        Err(TransportError::retryable(why))
    }

    /// Every held artefact whose dropped name is free returned under it,
    /// each refused one remembered; every one gone from under its claimed
    /// name forgotten. Nothing held costs one lock.
    pub fn settle<P: ClaimSite<Stamp = S> + ?Sized>(&self, site: &P) {
        let mut held = lock(&self.inner.held);
        held.retain(|claimed, file| {
            let Some(claim) = claim_of(claimed) else {
                return false;
            };
            match site.restore(claimed, claim.dropped) {
                Ok(Restore::Back) => {
                    if file.refused {
                        self.remember(site, claim.dropped.to_string());
                    }
                    false
                }
                Ok(Restore::Gone) => false,
                Ok(Restore::Kept(why)) => {
                    file.why = held_because(&why, claimed);
                    true
                }
                Err(failed) => {
                    file.why = held_because(&failed.message, claimed);
                    true
                }
            }
        });
    }

    /// What is held now, by claimed name.
    #[must_use]
    pub fn held(&self) -> Vec<Held> {
        lock(&self.inner.held).values().cloned().collect()
    }

    /// Every claim this claimant's name still holds at `site` returned, and
    /// every turn it left removed: the node is starting, so whatever it held
    /// before, nothing holds now. One whose dropped name a newer artefact
    /// has taken is held until the name is free.
    ///
    /// # Errors
    /// Where `site` could not be listed.
    pub fn recover<P: ClaimSite<Stamp = S> + ?Sized>(&self, site: &P) -> Result<()> {
        for claimed in self.inner.claimant.recover(site)? {
            // Held where it cannot go back now, and visible as such.
            let _ = self.give_back(site, &claimed, false);
        }
        Ok(())
    }

    /// `dropped` remembered as refused, as it lies now.
    fn remember<P: ClaimSite<Stamp = S> + ?Sized>(&self, site: &P, dropped: String) {
        if let Ok(Some(stamp)) = site.stamp(&dropped) {
            self.inner.refused.remember(dropped, stamp);
        }
    }
}

/// Why the claimed artefact `claimed` is held: `why`, and what becomes of
/// it.
fn held_because(why: &str, claimed: &str) -> String {
    format!("{why}; held as {claimed} and returned once its name is free")
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
