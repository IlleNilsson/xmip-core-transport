//! A sent file not yet in place: the name it is written under beside its
//! target, and its removal once the node that wrote it starts again.
//!
//! A technology of the file family that writes a sent file whole before
//! it is there — the file transport — writes it beside its target under a
//! name no listing takes, then renames it over the target. A process that
//! dies between the two leaves that file behind. The owner, 2026-10-19
//! (`open-problems.md` problem 39; ADR-0024, amendment 2026-10-19): **the
//! name carries the node's identity, not the process's, and at its start
//! the node removes its own leftovers while it recovers its claims. It
//! removes no other node's file.**
//!
//! The name is the target's, the mark `.xmip-sending.`, the claimant — the
//! node's location percent-encoded, as a claimed name carries it
//! ([`Claimant::of`]) — a count of this process's sends, so two at once
//! never share one, and the time in nanoseconds since the Unix epoch:
//!
//! ```text
//! order.edi.xmip-sending.xmip%3A%2F%2F%2F<cluster>%2Fnode%2F<node>.7.1790000000123456789
//! ```
//!
//! **A send in flight is never removed.** A node builds a Location's
//! transport, and so recovers, while its other Locations may be sending
//! into the same directory. Every name is held in this process's set of
//! sends in flight from before its file is created until the send has
//! renamed or removed it ([`Unfinished`]), and a removal passes over every
//! name the set holds.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;

use super::claimant::Claimant;
use super::site::ClaimSite;

/// What a sent file not yet in place carries between its target's name
/// and the claimant.
pub const SENDING: &str = ".xmip-sending.";

/// Sends named by this process so far.
static SENDS: AtomicU64 = AtomicU64::new(0);

/// The names this process's sends are writing now.
static IN_FLIGHT: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn in_flight() -> MutexGuard<'static, BTreeSet<String>> {
    IN_FLIGHT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The name one send writes under before it is renamed into place, held
/// as in flight until it is dropped: a send keeps it until its file is
/// renamed over the target or removed.
#[derive(Debug)]
pub struct Unfinished(String);

impl Unfinished {
    /// The name, beside the target.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        in_flight().remove(&self.0);
    }
}

impl Claimant {
    /// The name a send to `target`, a name in its directory, writes under
    /// before it is renamed into place; in flight until it is dropped.
    #[must_use]
    pub fn sending(&self, target: &str) -> Unfinished {
        let count = SENDS.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let name = format!("{target}{SENDING}{}.{count}.{nanos}", self.as_str());
        in_flight().insert(name.clone());
        Unfinished(name)
    }

    /// Of `names`, the names lying at `site`, every sent file this
    /// claimant's sends left unfinished is removed — never another's, and
    /// never one a send of this process is still writing. How many were
    /// removed.
    ///
    /// # Errors
    /// Where the far end refused a removal.
    pub fn remove_unfinished<P: ClaimSite + ?Sized>(
        &self,
        site: &P,
        names: impl IntoIterator<Item = String>,
    ) -> Result<usize> {
        let mut left: Vec<String> = names
            .into_iter()
            .filter(|name| sender_of(name) == Some(self.as_str()))
            .collect();
        // A name lying there and in flight is one a send is writing; one
        // not in flight now never will be, since no two sends share a name.
        {
            let writing = in_flight();
            left.retain(|name| !writing.contains(name));
        }
        for name in &left {
            site.delete(name)?;
        }
        Ok(left.len())
    }
}

/// The claimant whose send is writing, or left, the file `name`; nothing
/// where `name` is no sent file not yet in place.
///
/// Read from its end, a dot at a time, never searched: every name a
/// listing finds is asked, and most are not one.
#[must_use]
pub fn sender_of(name: &str) -> Option<&str> {
    let bytes = name.as_bytes();
    let counted = before(bytes, bytes.len(), u8::is_ascii_digit)?;
    let numbered = before(bytes, counted, u8::is_ascii_digit)?;
    let named = before(bytes, numbered, |byte| *byte != b'.')?;
    let target = name[..named].strip_suffix(&SENDING[..SENDING.len() - 1])?;
    (!target.is_empty()).then(|| &name[named + 1..numbered])
}

/// Where the dot lies before the run of `bytes` up to `end` that `kept`
/// takes: `None` where the run is empty or no dot ends it.
fn before(bytes: &[u8], end: usize, kept: impl Fn(&u8) -> bool) -> Option<usize> {
    let mut at = end;
    while at > 0 && kept(&bytes[at - 1]) {
        at -= 1;
    }
    (at < end && at > 0 && bytes[at - 1] == b'.').then(|| at - 1)
}

/// Whether `name` is a sent file not yet in place: never an arrival.
#[must_use]
pub fn is_unfinished(name: &str) -> bool {
    sender_of(name).is_some()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::super::claiming::Claiming;
    use super::super::memory::Memory;
    use super::*;
    use crate::node::NodeLocation;

    fn claimant(place: usize) -> Claimant {
        let node = NodeLocation::new(format!("process-{}/place-{place}", std::process::id()));
        Claimant::of(&node)
    }

    /// A file `claimant`'s send to `target` left behind: its name, no
    /// longer in flight.
    fn left(claimant: &Claimant, target: &str) -> String {
        claimant.sending(target).name().to_string()
    }

    #[test]
    fn a_starting_node_removes_what_its_sends_left_and_nothing_of_another() {
        let site = Memory::default();
        let (me, them) = (claimant(0), claimant(1));
        let (mine, theirs) = (left(&me, "order.edi"), left(&them, "order.edi"));
        for name in ["order.edi", &mine, &theirs] {
            site.drop_file(name, b"ISA");
        }

        me.recover(&site).expect("recovered");

        let after = site.names();
        assert!(!after.contains(&mine), "its own removed: {after:?}");
        assert!(after.contains(&theirs), "another node's kept: {after:?}");
        assert!(after.contains("order.edi"), "{after:?}");
    }

    #[test]
    fn a_send_in_flight_is_never_removed() {
        let site = Memory::default();
        let me = claimant(0);
        let writing = me.sending("order.edi");
        site.drop_file(writing.name(), b"ISA");

        me.recover(&site).expect("recovered");
        assert!(site.names().contains(writing.name()), "still being written");

        let name = writing.name().to_string();
        drop(writing);
        me.recover(&site).expect("recovered again");
        assert!(!site.names().contains(&name), "left behind, it is removed");
    }

    #[test]
    fn a_sent_file_not_in_place_is_never_an_arrival() {
        let site = Memory::default();
        let them = claimant(1);
        site.drop_file(&left(&them, "order.edi"), b"ISA");
        let claims = Claiming::new(claimant(0));
        for _ in 0..3 {
            assert_eq!(claims.take(&site, usize::MAX).expect("listed"), []);
        }
    }

    #[test]
    fn only_a_sent_file_not_in_place_reads_as_one() {
        let me = claimant(0);
        let named = left(&me, "in.order.edi");
        assert_eq!(sender_of(&named), Some(me.as_str()), "{named}");
        assert!(named.starts_with("in.order.edi.xmip-sending."), "{named}");
        assert!(!is_unfinished("order.edi"));
        assert!(!is_unfinished(".xmip-sending.node.1.2"));
        assert!(!is_unfinished("order.edi.xmip-sending.node.1"));
        assert!(!is_unfinished("order.edi.xmip-sending.node.x.2"));
        assert!(!is_unfinished("order.edi.xmip-sending.node.1.2.3"));
        assert!(!is_unfinished("order.edi.xmip-sending..1.2"));
    }

    /// A location of a hundred names: thirty this node's sends left,
    /// thirty another's, forty dropped files.
    fn crowded() -> Memory {
        let site = Memory::default();
        let (me, them) = (claimant(0), claimant(1));
        for file in 0..30 {
            site.drop_file(&left(&me, &format!("mine-{file}.edi")), b"ISA");
            site.drop_file(&left(&them, &format!("theirs-{file}.edi")), b"ISA");
        }
        for file in 0..40 {
            site.drop_file(&format!("order-{file}.edi"), b"ISA");
        }
        site
    }

    /// What a recovery costs beyond the far end's own work — its listings
    /// and each removal it is asked for — over a hundred names: the
    /// fastest of five rounds, each over a location made anew.
    #[test]
    fn the_removal_costs_no_more_than_a_millisecond_beyond_the_far_end() {
        let me = claimant(0);
        let rounds = (0..5).map(|_| {
            let site = crowded();
            let before = site.spent();
            let started = Instant::now();
            me.recover(&site).expect("recovered");
            let all = started.elapsed();
            let far_end = site.spent().saturating_sub(before);
            assert_eq!(site.names().len(), 70, "thirty removed");
            (all.saturating_sub(far_end), all, far_end)
        });
        let (beyond, all, far_end) = rounds.min().expect("five rounds");
        assert!(
            beyond <= Duration::from_millis(1),
            "{beyond:?} beyond the far end ({all:?} in all, {far_end:?} at the far end)"
        );
    }
}
