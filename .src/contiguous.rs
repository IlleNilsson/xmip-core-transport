//! A place in an ordered log that a verdict moves only contiguously.
//!
//! A log read on from a place — a Kafka partition from an offset, a Redis
//! Stream after an id, a Kinesis shard after a sequence number — consumes
//! nothing as it is read: the place is the reader's. It moves as the
//! runtime accepts what was read, and only from where it stands: accepting
//! the record after `from` moves it from `from` to that record, and only
//! where it stands at `from`. A record refused for good moves it the same
//! way: it is not read again. A failed record leaves it, so that record and
//! every one read after it are read again — at least once, never a skip —
//! and a record accepted after one failed moves nothing.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::acknowledgement::{Acknowledgement, Verdict};

/// The place a reader reads on from, shared with the acknowledgements of
/// what it read. A clone shares the place.
pub struct Contiguous<P>(Arc<Mutex<P>>);

impl<P> Contiguous<P> {
    /// A place standing at `at`.
    #[must_use]
    pub fn new(at: P) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    /// Where the place stands now.
    #[must_use]
    pub fn at(&self) -> P
    where
        P: Clone,
    {
        self.held().clone()
    }

    /// Stand the place at `at`, whatever was accepted: where a Location
    /// starts reading.
    pub fn set(&self, at: P) {
        *self.held() = at;
    }

    /// Move the place from `from` to `to`, only where it stands at `from`,
    /// and say whether it moved.
    pub fn advance(&self, from: &P, to: P) -> bool
    where
        P: PartialEq,
    {
        let mut at = self.held();
        let moves = *at == *from;
        if moves {
            *at = to;
        }
        moves
    }

    /// The acknowledgement of the record read after `from` that moves the
    /// place to `to`: [`Verdict::Accepted`] and [`Verdict::Refused`] — a
    /// record refused for good is not read again — advance it, only where
    /// it stands at `from`; [`Verdict::Failed`] leaves it. An in-memory
    /// step, no round trip.
    #[must_use]
    pub fn advancing(&self, from: P, to: P) -> Acknowledgement
    where
        P: PartialEq + Send + 'static,
    {
        let place = self.clone();
        Acknowledgement::deferred(move |verdict| {
            if verdict != Verdict::Failed {
                place.advance(&from, to);
            }
            Ok(())
        })
    }

    fn held(&self) -> MutexGuard<'_, P> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<P> Clone for Contiguous<P> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<P: Default> Default for Contiguous<P> {
    fn default() -> Self {
        Self::new(P::default())
    }
}

impl<P: fmt::Debug> fmt::Debug for Contiguous<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Contiguous").field(&*self.held()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_in_order_the_place_moves_record_by_record() {
        let place = Contiguous::new(4_i64);
        for offset in 4..7 {
            place
                .advancing(offset, offset + 1)
                .acknowledge(Verdict::Accepted)
                .expect("told");
        }
        assert_eq!(place.at(), 7);
    }

    #[test]
    fn a_failed_record_holds_the_place_for_itself_and_what_follows() {
        let place = Contiguous::new("1-0".to_string());
        let failed = place.advancing("1-0".into(), "2-0".into());
        let after = place.advancing("2-0".into(), "3-0".into());
        failed.acknowledge(Verdict::Failed).expect("told");
        after.acknowledge(Verdict::Accepted).expect("told");
        assert_eq!(place.at(), "1-0", "never a skip past 2-0");
        place
            .advancing("1-0".into(), "2-0".into())
            .acknowledge(Verdict::Accepted)
            .expect("told");
        assert_eq!(place.at(), "2-0");
    }

    #[test]
    fn a_refused_record_moves_the_place_past_it() {
        let place = Contiguous::new(1_i64);
        place
            .advancing(1, 2)
            .acknowledge(Verdict::Refused(crate::Refusal::Unacceptable))
            .expect("told");
        assert_eq!(place.at(), 2, "refused for good, not read again");
    }

    #[test]
    fn a_place_set_or_advanced_by_hand_says_whether_it_moved() {
        let place: Contiguous<Option<String>> = Contiguous::default();
        assert!(place.advance(&None, Some("1".into())));
        assert!(!place.advance(&None, Some("9".into())), "not at None now");
        place.set(Some("5".into()));
        assert_eq!(place.at().as_deref(), Some("5"));
        assert_eq!(format!("{place:?}"), r#"Contiguous(Some("5"))"#);
        assert_eq!(place.clone().at(), place.at(), "a clone shares the place");
    }
}
