//! A location of the file family held in memory, to prove the claim
//! sequence on: names to bytes and stamps, its turns, and a clock that
//! counts its writes. Every operation is made under one lock, as a far end
//! makes each of its own atomically, and the time the far end spends on
//! them is counted, so a test measures what the claim does around it.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::error::Result;

use super::site::{ClaimSite, Listed, Restore};

#[derive(Default)]
struct Held {
    /// Each file: its bytes, and the write that last stamped it.
    files: BTreeMap<String, (Vec<u8>, u64)>,
    /// Each turn and its holder.
    turns: BTreeMap<String, String>,
    writes: u64,
    /// Names whose rename the far end refuses, as it does a held file.
    refusing: BTreeSet<String>,
    /// The time spent in the far end's operations so far.
    spent: Duration,
}

/// The location; cloned, the same one.
#[derive(Clone, Default)]
pub struct Memory(Arc<Mutex<Held>>);

/// One operation at the far end: its time added to what the far end spent
/// as it ends.
struct Operation<'a> {
    held: MutexGuard<'a, Held>,
    started: Instant,
}

impl Deref for Operation<'_> {
    type Target = Held;
    fn deref(&self) -> &Held {
        &self.held
    }
}

impl DerefMut for Operation<'_> {
    fn deref_mut(&mut self) -> &mut Held {
        &mut self.held
    }
}

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        let spent = self.started.elapsed();
        self.held.spent += spent;
    }
}

impl Memory {
    fn held(&self) -> Operation<'_> {
        let started = Instant::now();
        let held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        Operation { held, started }
    }

    /// The time spent in the far end's operations so far.
    pub fn spent(&self) -> Duration {
        self.held().spent
    }

    /// `name` written as `bytes`.
    pub fn drop_file(&self, name: &str, bytes: &[u8]) {
        let mut held = self.held();
        held.writes += 1;
        let stamp = held.writes;
        held.files.insert(name.to_string(), (bytes.to_vec(), stamp));
    }

    /// The names lying here, turns too.
    pub fn names(&self) -> BTreeSet<String> {
        let held = self.held();
        held.files
            .keys()
            .chain(held.turns.keys())
            .cloned()
            .collect()
    }

    /// The bytes of `name`.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.held().files.get(name).map(|(bytes, _)| bytes.clone())
    }

    /// The far end refuses to rename `name` from now on.
    pub fn refuse_renaming(&self, name: &str) {
        self.held().refusing.insert(name.to_string());
    }
}

impl ClaimSite for Memory {
    type Stamp = u64;

    fn listed(&self) -> Result<Vec<Listed<u64>>> {
        let held = self.held();
        Ok(held
            .files
            .iter()
            .map(|(name, (_, stamp))| (name.clone(), Some(*stamp)))
            .collect())
    }

    fn turns(&self) -> Result<Vec<String>> {
        Ok(self.held().turns.keys().cloned().collect())
    }

    fn stamp(&self, name: &str) -> Result<Option<u64>> {
        Ok(self.held().files.get(name).map(|(_, stamp)| *stamp))
    }

    fn take_turn(&self, turn: &str, claimant: &str) -> Result<bool> {
        let mut held = self.held();
        if held.files.contains_key(turn) || held.turns.contains_key(turn) {
            return Ok(false);
        }
        held.turns.insert(turn.to_string(), claimant.to_string());
        Ok(true)
    }

    fn holder(&self, turn: &str) -> Result<Option<String>> {
        Ok(self.held().turns.get(turn).cloned())
    }

    fn drop_turn(&self, turn: &str, _claimant: &str) -> Result<()> {
        self.held().turns.remove(turn);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<bool> {
        let mut held = self.held();
        if held.refusing.contains(from) {
            return Ok(false);
        }
        Ok(match held.files.remove(from) {
            Some(file) => {
                held.files.insert(to.to_string(), file);
                true
            }
            None => false,
        })
    }

    fn restore(&self, claimed: &str, dropped: &str) -> Result<Restore> {
        let mut held = self.held();
        if held.files.contains_key(dropped) {
            return Ok(Restore::Kept(format!(
                "returning {dropped}: a file of that name was dropped since"
            )));
        }
        Ok(match held.files.remove(claimed) {
            Some(file) => {
                held.files.insert(dropped.to_string(), file);
                Restore::Back
            }
            None => Restore::Gone,
        })
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.held().files.remove(name);
        Ok(())
    }
}
