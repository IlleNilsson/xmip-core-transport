//! The keys far ends presented on first contact, kept so that every later
//! contact is held to them. Decided 2026-10-28 for SFTP: a server's host key
//! is pinned on first contact, with no confirmation step (ADR-0019,
//! amendment 2026-10-28).
//!
//! A technology that authenticates its far end by a key no anchor vouches
//! for — an SSH server's host key — asks [`Pinned::hold`] with the far end's
//! name and the key's fingerprint as it connects, before it presents any
//! credential: the first fingerprint a name presents is pinned, the same one
//! again is held, and another is refused in words naming both, as a key not
//! the far end's own. The runtime hands every transport it builds the
//! node's pins, kept in one file under its data directory
//! ([`Pinned::kept_at`], `Configured::pinned_by`), so a node started again
//! holds its far ends to the keys they presented before; a transport built
//! outside a node pins in memory, for its own life ([`Pinned::in_memory`]).
//!
//! The file is one line a far end, `<fingerprint> <name>`, appended as each
//! is pinned. A far end whose key was changed on purpose is pinned again by
//! deleting its line.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::error::{Result, TransportError};

/// The far ends' pinned keys: in memory, or kept in a file.
#[derive(Debug)]
pub struct Pinned {
    /// Where they are kept; `None` in memory.
    file: Option<PathBuf>,
    /// Each far end's fingerprint, by name; read from the file at the first
    /// hold.
    held: Mutex<Option<BTreeMap<String, String>>>,
}

impl Pinned {
    /// Pins kept for this process's life.
    #[must_use]
    pub const fn in_memory() -> Self {
        Self {
            file: None,
            held: Mutex::new(None),
        }
    }

    /// Pins kept in the file at `file`, read at the first hold and appended
    /// to as each far end is pinned.
    #[must_use]
    pub fn kept_at(file: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(file.into()),
            held: Mutex::new(None),
        }
    }

    /// Where the pins are kept, where they are kept in a file.
    #[must_use]
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Hold the far end `name` to `fingerprint`: pin it where nothing is
    /// pinned for `name`, accept it where it is the one pinned.
    ///
    /// # Errors
    /// A permanent failure where another fingerprint is pinned for `name`,
    /// naming both; where the file could not be read or written.
    pub fn hold(&self, name: &str, fingerprint: &str) -> Result<()> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            *held = Some(self.read()?);
        }
        let pins = held.get_or_insert_with(BTreeMap::new);
        match pins.get(name) {
            Some(pinned) if pinned == fingerprint => Ok(()),
            Some(pinned) => Err(TransportError::permanent(format!(
                "the far end {name} presented the key {fingerprint}, and {pinned} is pinned \
                 for it{}: refused, as a key not its own",
                self.file
                    .as_deref()
                    .map(|file| format!(" in {}", file.display()))
                    .unwrap_or_default()
            ))),
            None => {
                self.append(name, fingerprint)?;
                pins.insert(name.to_string(), fingerprint.to_string());
                Ok(())
            }
        }
    }

    /// The pins the file holds; none where there is no file yet.
    fn read(&self) -> Result<BTreeMap<String, String>> {
        let Some(file) = &self.file else {
            return Ok(BTreeMap::new());
        };
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(absent) if absent.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(unread) => return Err(unkept(file, &unread)),
        };
        Ok(text
            .lines()
            .filter_map(|line| line.trim().split_once(' '))
            .map(|(fingerprint, name)| (name.trim().to_string(), fingerprint.to_string()))
            .collect())
    }

    /// Append `name`'s pin to the file, synced, where there is one.
    fn append(&self, name: &str, fingerprint: &str) -> Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(directory) = file.parent() {
            std::fs::create_dir_all(directory).map_err(|unmade| unkept(file, &unmade))?;
        }
        let mut kept = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .map_err(|unopened| unkept(file, &unopened))?;
        kept.write_all(format!("{fingerprint} {name}\n").as_bytes())
            .and_then(|()| kept.sync_all())
            .map_err(|unwritten| unkept(file, &unwritten))
    }
}

impl Default for Pinned {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// The failure to read or write the pins' file, retryable: the next contact
/// asks again.
fn unkept(file: &Path, error: &std::io::Error) -> TransportError {
    TransportError::retryable(format!(
        "the pinned keys at {} could not be kept: {error}",
        file.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_key_is_pinned_the_same_held_and_another_refused_naming_both() {
        let pins = Pinned::in_memory();
        pins.hold("sftp://host:22", "SHA256:one").expect("pinned");
        pins.hold("sftp://host:22", "SHA256:one").expect("held");
        pins.hold("sftp://other:22", "SHA256:two").expect("its own");
        let refused = pins
            .hold("sftp://host:22", "SHA256:two")
            .expect_err("not its own");
        assert!(!refused.retryable);
        assert!(
            refused.message.contains("SHA256:two") && refused.message.contains("SHA256:one"),
            "{}",
            refused.message
        );
    }

    #[test]
    fn pins_kept_in_a_file_hold_a_far_end_after_a_restart() {
        let directory = std::env::temp_dir().join(format!("xmip-pinned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let file = directory.join("pinned-keys");
        Pinned::kept_at(&file)
            .hold("sftp://host:22", "SHA256:one")
            .expect("pinned");
        let again = Pinned::kept_at(&file);
        let refused = again
            .hold("sftp://host:22", "SHA256:two")
            .expect_err("pinned before the restart");
        assert!(refused.message.contains(&file.display().to_string()));
        again.hold("sftp://host:22", "SHA256:one").expect("held");
        assert_eq!(
            std::fs::read_to_string(&file).expect("kept"),
            "SHA256:one sftp://host:22\n"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
