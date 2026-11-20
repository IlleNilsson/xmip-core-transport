//! An object store held in memory, for the claim's tests: it honors each
//! condition as Amazon S3, Azure Blob Storage and Google Cloud Storage
//! document theirs. A get or a delete naming a stamp the object no longer
//! bears, or an object gone, is answered `Changed` (`If-Match`,
//! `ifGenerationMatch`: `412`, `404`); a create where an object lies is
//! answered `None` and writes nothing (`If-None-Match: *`,
//! `ifGenerationMatch=0`: `412`, or Azure Blob's `409`), and the store
//! decides between two creates of one name alone, under one lock, as each
//! service does.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use xcore::lock::Locked;

use crate::error::{Result, TransportError};

use super::{AsListed, Object, Store};

/// The store. Cloned, the same objects.
#[derive(Clone, Default)]
pub(super) struct Memory {
    held: Arc<Mutex<Held>>,
}

#[derive(Default)]
struct Held {
    /// The objects by name, each with its bytes and its stamp.
    objects: BTreeMap<String, (Vec<u8>, String)>,
    /// How many writes were made: each write's stamp.
    writes: u64,
    /// What was asked, in order: `list`, `get <name>`, `delete <name>`,
    /// `create <name>`.
    asked: Vec<String>,
    /// The act that fails, `delete` say, with why.
    failing: Option<(String, String)>,
}

impl Memory {
    /// A store holding `objects`, each written once.
    pub(super) fn holding(objects: &[(&str, &[u8])]) -> Self {
        let store = Self::default();
        for (name, bytes) in objects {
            store.write(name, bytes);
        }
        store
    }

    /// `name` written, over whatever lay there: its stamp.
    pub(super) fn write(&self, name: &str, bytes: &[u8]) -> String {
        let mut held = self.held.locked();
        held.writes += 1;
        let stamp = held.writes.to_string();
        held.objects
            .insert(name.to_string(), (bytes.to_vec(), stamp.clone()));
        stamp
    }

    /// `name` gone, deleted by another client.
    pub(super) fn remove(&self, name: &str) {
        self.held.locked().objects.remove(name);
    }

    /// The names held now.
    pub(super) fn names(&self) -> Vec<String> {
        self.held.locked().objects.keys().cloned().collect()
    }

    /// What `name` holds now.
    pub(super) fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.held.locked().objects.get(name).map(|(b, _)| b.clone())
    }

    /// What was asked so far, emptied.
    pub(super) fn asked(&self) -> Vec<String> {
        std::mem::take(&mut self.held.locked().asked)
    }

    /// Every `act` failing from now, for `why`; `None`, nothing failing.
    pub(super) fn failing(&self, act: Option<(&str, &str)>) {
        self.held.locked().failing = act.map(|(act, why)| (act.to_string(), why.to_string()));
    }

    fn ask(&self, act: &str, name: &str) -> Result<std::sync::MutexGuard<'_, Held>> {
        let mut held = self.held.locked();
        let asked = if name.is_empty() {
            act.to_string()
        } else {
            format!("{act} {name}")
        };
        held.asked.push(asked);
        match &held.failing {
            Some((failing, why)) if failing == act => Err(TransportError::retryable(why.clone())),
            _ => Ok(held),
        }
    }
}

impl Store for Memory {
    fn list(&self) -> Result<Vec<Object>> {
        let held = self.ask("list", "")?;
        Ok(held
            .objects
            .iter()
            .map(|(name, (_, stamp))| (name.clone(), stamp.clone()))
            .collect())
    }

    fn get(&self, name: &str, stamp: &str) -> Result<AsListed<Vec<u8>>> {
        let held = self.ask("get", name)?;
        Ok(match held.objects.get(name) {
            Some((bytes, lying)) if lying == stamp => AsListed::Held(bytes.clone()),
            _ => AsListed::Changed,
        })
    }

    fn delete(&self, name: &str, stamp: &str) -> Result<AsListed<()>> {
        let mut held = self.ask("delete", name)?;
        if held
            .objects
            .get(name)
            .is_none_or(|(_, lying)| lying != stamp)
        {
            return Ok(AsListed::Changed);
        }
        held.objects.remove(name);
        Ok(AsListed::Held(()))
    }

    fn create(&self, name: &str, bytes: &[u8]) -> Result<Option<String>> {
        let mut held = self.ask("create", name)?;
        if held.objects.contains_key(name) {
            return Ok(None);
        }
        held.writes += 1;
        let stamp = held.writes.to_string();
        held.objects
            .insert(name.to_string(), (bytes.to_vec(), stamp.clone()));
        Ok(Some(stamp))
    }

    fn origin(&self, name: &str) -> String {
        format!("memory://inbox/{name}")
    }
}
