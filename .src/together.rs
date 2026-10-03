//! Arrivals whose far end settles them together, once the last of them has
//! its verdict.
//!
//! Some protocols answer a receive whole rather than message by message:
//! POP3 commits its deletes at `QUIT`, an MQ unit of work commits or backs
//! out every get at once, Event Grid reads one status for a batch, IMAP
//! expunges once, a SQL receive claims its accepted rows in one
//! transaction. Each arrival still gets its own acknowledgement; this
//! tallies them and settles once every one has a verdict — or was dropped
//! without one, which is told as `None`.

use std::sync::{Arc, Mutex, PoisonError};

use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::error::Result;

/// What the last verdict settles: every arrival's verdict, by its place,
/// `None` where its acknowledgement was dropped without one.
type Settle = Box<dyn FnOnce(&[Option<Verdict>]) -> Result<()> + Send>;

struct Tally {
    verdicts: Vec<Option<Verdict>>,
    left: usize,
    settle: Option<Settle>,
}

/// One acknowledgement for each of `count` arrivals. `each` runs for an
/// arrival as its verdict is given — a `DELE`, a flag — with its place;
/// `settle` runs once, after the last verdict or drop, with them all, and
/// its result is the last verdict's. A failed `each` still counts its
/// arrival; where the last one is dropped, `settle` runs and its failure
/// has nobody to tell.
#[must_use]
pub fn together(
    count: usize,
    each: impl Fn(usize, Verdict) -> Result<()> + Send + Sync + 'static,
    settle: impl FnOnce(&[Option<Verdict>]) -> Result<()> + Send + 'static,
) -> Vec<Acknowledgement> {
    let tally = Arc::new(Mutex::new(Tally {
        verdicts: vec![None; count],
        left: count,
        settle: Some(Box::new(settle)),
    }));
    let each = Arc::new(each);
    (0..count)
        .map(|at| {
            let mut place = Place {
                tally: Some(Arc::clone(&tally)),
                at,
            };
            let each = Arc::clone(&each);
            Acknowledgement::deferred(move |verdict| {
                let told = each(at, verdict);
                let settled = place.counted(Some(verdict));
                told.and(settled)
            })
        })
        .collect()
}

/// One arrival's place in the tally; dropped uncounted, it counts as
/// `None`.
struct Place {
    tally: Option<Arc<Mutex<Tally>>>,
    at: usize,
}

impl Place {
    fn counted(&mut self, verdict: Option<Verdict>) -> Result<()> {
        let Some(tally) = self.tally.take() else {
            return Ok(());
        };
        let (settle, verdicts) = {
            let mut tally = tally.lock().unwrap_or_else(PoisonError::into_inner);
            tally.verdicts[self.at] = verdict;
            tally.left -= 1;
            if tally.left > 0 {
                return Ok(());
            }
            (tally.settle.take(), std::mem::take(&mut tally.verdicts))
        };
        settle.map_or(Ok(()), |settle| settle(&verdicts))
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        drop(self.counted(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_last_verdict_settles_them_all_and_a_dropped_one_counts_as_none() {
        let settled = Arc::new(Mutex::new(None));
        let into = Arc::clone(&settled);
        let told = Arc::new(AtomicUsize::new(0));
        let each_told = Arc::clone(&told);
        let mut acknowledgements = together(
            3,
            move |_, _| {
                each_told.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move |verdicts| {
                *into.lock().expect("lock") = Some(verdicts.to_vec());
                Ok(())
            },
        );
        let last = acknowledgements.pop().expect("three");
        let dropped = acknowledgements.pop().expect("two");
        let first = acknowledgements.pop().expect("one");
        first.acknowledge(Verdict::Accepted).expect("told");
        drop(dropped);
        assert!(settled.lock().expect("lock").is_none(), "one still waits");
        last.acknowledge(Verdict::Failed).expect("settled");
        assert_eq!(
            *settled.lock().expect("lock"),
            Some(vec![Some(Verdict::Accepted), None, Some(Verdict::Failed)])
        );
        assert_eq!(told.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_settling_failure_is_the_last_verdicts() {
        let mut acknowledgements = together(
            1,
            |_, _| Ok(()),
            |_| Err(crate::error::protocol_error("the commit failed")),
        );
        let error = acknowledgements
            .remove(0)
            .acknowledge(Verdict::Accepted)
            .expect_err("failed");
        assert_eq!(error.message, "the commit failed");
    }
}
