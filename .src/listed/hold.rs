//! A claimed object held through its receive cycle: told its verdict
//! once, its claim let go however the cycle ends.

use std::sync::Arc;

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::error::Result;

use super::{Claims, Object, Store};

/// One object a node won, from its listing until its verdict: the object
/// as listed, and its claim as written.
pub(super) struct Hold {
    claims: Claims,
    store: Arc<dyn Store>,
    object: Object,
    /// The claim's name, `<name>.xmip-claim`, and the stamp it was
    /// written with.
    claim: Object,
    told: bool,
}

impl Hold {
    /// The acknowledgement of `object`, claimed at `store` as `claim`:
    /// on `Accepted` the object deleted as it was listed, then the claim
    /// let go; on `Refused` and `Failed` the claim let go and the object
    /// left. Let go without a verdict, the claim is let go too.
    pub(super) fn acknowledgement(
        claims: Claims,
        store: Arc<dyn Store>,
        object: Object,
        claim: Object,
    ) -> Acknowledgement {
        let hold = Self {
            claims,
            store,
            object,
            claim,
            told: false,
        };
        Acknowledgement::deferred(move |verdict| hold.tell(verdict))
    }

    fn tell(mut self, verdict: Verdict) -> Result<()> {
        self.told = true;
        // The object first: a claim let go before it is deleted would let
        // another node read it again.
        let consumed = match verdict {
            // Gone since, or written again: what lies there now is the
            // next listing's, and what was accepted is done with.
            Verdict::Accepted => {
                let (name, stamp) = &self.object;
                self.store.delete(name, stamp).map(|_| ())
            }
            Verdict::Refused(_) | Verdict::Failed => Ok(()),
        };
        // Let go even where the delete failed: the object is then
        // received again, at least once, never held by nobody.
        let released = self.let_go();
        consumed.and(released)
    }

    fn let_go(&self) -> Result<()> {
        let (claim, stamp) = &self.claim;
        self.claims.let_go(&*self.store, claim, stamp)
    }
}

impl Drop for Hold {
    /// Let go without a verdict: nothing was consumed, and nothing holds
    /// the object now.
    fn drop(&mut self) {
        if !self.told {
            let _ = self.let_go();
        }
    }
}
