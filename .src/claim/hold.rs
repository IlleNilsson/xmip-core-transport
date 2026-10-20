//! A claimed artifact held through its receive cycle: told its verdict
//! once, and returned where it was dropped if it never is.

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::error::Result;

use super::claiming::Claiming;
use super::site::ClaimSite;

impl<S: Clone + Eq + Send + 'static> Claiming<S> {
    /// The acknowledgement of the artifact claimed as `claimed` at `site`:
    /// the verdict told as [`Claiming::told`] tells it, deleting on
    /// `Accepted` where `consume` says; let go without one, it is returned.
    #[must_use]
    pub fn acknowledgement<P>(&self, site: P, claimed: String, consume: bool) -> Acknowledgement
    where
        P: ClaimSite<Stamp = S> + Send + 'static,
    {
        let hold = Hold {
            claiming: self.clone(),
            site,
            claimed,
            consume,
            told: false,
        };
        Acknowledgement::deferred(move |verdict| hold.tell(verdict))
    }
}

struct Hold<S: Clone + Eq + Send + 'static, P: ClaimSite<Stamp = S>> {
    claiming: Claiming<S>,
    site: P,
    claimed: String,
    consume: bool,
    told: bool,
}

impl<S: Clone + Eq + Send + 'static, P: ClaimSite<Stamp = S>> Hold<S, P> {
    fn tell(mut self, verdict: Verdict) -> Result<()> {
        self.told = true;
        self.claiming
            .told(&self.site, &self.claimed, Some(verdict), self.consume)
    }
}

impl<S: Clone + Eq + Send + 'static, P: ClaimSite<Stamp = S>> Drop for Hold<S, P> {
    /// Let go without a verdict: nothing was consumed, so the artifact goes
    /// back where it was dropped.
    fn drop(&mut self) {
        if !self.told {
            let _ = self
                .claiming
                .told(&self.site, &self.claimed, None, self.consume);
        }
    }
}
