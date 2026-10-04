//! What a Receive Location refused and left where it lies, each as it was
//! refused.
//!
//! A refusal is not a consumption ([`Verdict::Refused`]): where the far end
//! holds the only copy — a file in a drop directory or on a server, an
//! object in a bucket, a row, a mail — a refusal leaves it there. Left
//! there, the next receive lists it again, and it would be refused again
//! for as long as it lies there. So the Location remembers each refused
//! item by its name with its stamp — what says it is unchanged: a length
//! and a modification time, an `ETag`, a hash of a row's body — and a
//! listing leaves out every item that still lies as it was refused. One
//! written again under its name is another stamp, and a new arrival. A
//! refusal whose item is gone from a listing, or changed, is forgotten
//! there, so the memory never holds more than the far end does.
//!
//! The memory is the process's, as the file transport's always was: a node
//! that starts again receives a refused item once more, refuses it once
//! more, and remembers it from then on. Nothing at the far end is marked.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::acknowledgement::{Acknowledgement, Verdict};

/// The items a Location refused, by name, each with the stamp it had.
/// Cloned, it is the same memory: a transport and the acknowledgements it
/// hands out share it.
pub struct Refused<K, S> {
    held: Arc<Mutex<HashMap<K, S>>>,
}

impl<K, S> Clone for Refused<K, S> {
    fn clone(&self) -> Self {
        Self {
            held: Arc::clone(&self.held),
        }
    }
}

impl<K, S> Default for Refused<K, S> {
    fn default() -> Self {
        Self {
            held: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<K, S> std::fmt::Debug for Refused<K, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Refused({})", lock(&self.held).len())
    }
}

impl<K, S> Refused<K, S>
where
    K: Eq + Hash + Send + 'static,
    S: Eq + Send + 'static,
{
    /// `listing` without every item that lies as it was refused, `name`
    /// reading each item's name and `stamp` its stamp as it lies now — or
    /// `None` where it cannot be read, which is a changed item. `stamp` is
    /// asked only of an item whose name was refused, so a technology whose
    /// listing names files without their stamps asks the far end for the
    /// few it needs. Refusals not listed, or listed with another stamp, are
    /// forgotten. The memory is not locked while `stamp` asks.
    pub fn sift<T>(
        &self,
        listing: Vec<T>,
        name: impl Fn(&T) -> &K,
        mut stamp: impl FnMut(&T) -> Option<S>,
    ) -> Vec<T> {
        let mut held = std::mem::take(&mut *lock(&self.held));
        let mut still = HashMap::new();
        let mut kept = Vec::with_capacity(listing.len());
        for item in listing {
            if let Some((name, was)) = held.remove_entry(name(&item))
                && stamp(&item).as_ref() == Some(&was)
            {
                still.insert(name, was);
                continue;
            }
            kept.push(item);
        }
        // What was refused while this listing asked is kept too, and is
        // the newer stamp.
        let mut now = lock(&self.held);
        for (name, was) in still {
            now.entry(name).or_insert(was);
        }
        kept
    }

    /// Remember `name` refused as `stamp`.
    pub fn remember(&self, name: K, stamp: S) {
        lock(&self.held).insert(name, stamp);
    }

    /// `told`, which leaves its item where it lies on a refusal, remembering
    /// `name` as `stamp` when the cycle refuses it. An at-most-once
    /// acknowledgement is handed back as it is: nothing lies anywhere.
    #[must_use]
    pub fn remembering(&self, name: K, stamp: S, told: Acknowledgement) -> Acknowledgement {
        if !told.defers() {
            return told;
        }
        let refused = self.clone();
        Acknowledgement::deferred(move |verdict| {
            if matches!(verdict, Verdict::Refused(_)) {
                refused.remember(name, stamp);
            }
            told.acknowledge(verdict)
        })
    }

    /// How many refusals are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.held).len()
    }

    /// Whether nothing refused is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acknowledgement::Refusal;

    fn listing(items: &[(&str, u32)]) -> Vec<(String, u32)> {
        items.iter().map(|(n, s)| ((*n).to_string(), *s)).collect()
    }

    fn sifted(refused: &Refused<String, u32>, items: &[(&str, u32)]) -> Vec<String> {
        refused
            .sift(listing(items), |(name, _)| name, |(_, stamp)| Some(*stamp))
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn a_refused_item_is_left_out_while_unchanged_and_forgotten_once_changed_or_gone() {
        let refused = Refused::default();
        refused.remember("one.xml".to_string(), 1);
        refused.remember("two.xml".to_string(), 2);
        assert_eq!(
            sifted(
                &refused,
                &[("one.xml", 1), ("two.xml", 2), ("three.xml", 3)]
            ),
            ["three.xml"]
        );
        assert_eq!(refused.len(), 2);
        assert_eq!(
            sifted(&refused, &[("one.xml", 9)]),
            ["one.xml"],
            "written again"
        );
        assert!(
            refused.is_empty(),
            "one.xml changed, two.xml gone: both forgotten"
        );
        assert_eq!(
            sifted(&refused, &[("one.xml", 1), ("two.xml", 2)]),
            ["one.xml", "two.xml"]
        );
    }

    #[test]
    fn a_stamp_is_asked_only_of_a_refused_name_and_none_is_a_change() {
        let refused = Refused::default();
        refused.remember("one.xml".to_string(), 1);
        refused.remember("two.xml".to_string(), 2);
        let mut asked = Vec::new();
        let kept = refused.sift(
            vec![
                "one.xml".to_string(),
                "two.xml".to_string(),
                "three.xml".to_string(),
            ],
            |name| name,
            |name| {
                asked.push(name.clone());
                (name == "one.xml").then_some(1)
            },
        );
        assert_eq!(asked, ["one.xml", "two.xml"], "three.xml was never refused");
        assert_eq!(
            kept,
            ["two.xml", "three.xml"],
            "two.xml could not be stamped: changed"
        );
        assert_eq!(refused.len(), 1);
    }

    #[test]
    fn only_a_refusal_is_remembered_and_every_verdict_is_still_told() {
        let refused: Refused<String, u32> = Refused::default();
        let told = Arc::new(Mutex::new(Vec::new()));
        for (name, verdict) in [
            ("one.xml", Verdict::Accepted),
            ("two.xml", Verdict::Refused(Refusal::Forbidden)),
            ("three.xml", Verdict::Failed),
        ] {
            let into = Arc::clone(&told);
            let inner = Acknowledgement::deferred(move |verdict| {
                into.lock().expect("lock").push(verdict);
                Ok(())
            });
            let acknowledgement = refused.remembering(name.to_string(), 7, inner);
            assert!(acknowledgement.defers());
            acknowledgement.acknowledge(verdict).expect("told");
        }
        assert_eq!(told.lock().expect("lock").len(), 3);
        assert_eq!(
            sifted(
                &refused,
                &[("one.xml", 7), ("two.xml", 7), ("three.xml", 7)]
            ),
            ["one.xml", "three.xml"]
        );
        let datagram = Acknowledgement::at_most_once("a datagram");
        let once = refused.remembering("four.xml".to_string(), 1, datagram);
        assert!(
            !once.defers(),
            "an at-most-once protocol stays at-most-once"
        );
    }
}
