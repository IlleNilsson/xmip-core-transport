//! The operations a claim makes at the far end, in a location's own terms.
//!
//! ADR-0024, amendments 2026-09-26 and 2026-10-13: a file is claimed by
//! taking its turn, created only where nothing of that name exists, then
//! renaming it to a name recording the claimant; the file family — a local
//! or shared directory, FTP, FTPS, SFTP — offers both steps, each in its
//! own words. A [`ClaimSite`] is one such location, addressed by the names
//! in it; the sequence itself, the names and the recovery are written once,
//! in [`super::Claimant`] and [`super::Claiming`], and every technology of
//! the family implements only these.
//!
//! | operation | file system | FTP, FTPS | SFTP |
//! | --- | --- | --- | --- |
//! | list | read the directory | `MLSD`, or `NLST` with `SIZE` and `MDTM` | `READDIR` |
//! | take a turn | create a new file (see below) | `MKD` | `OPEN` with `CREAT` and `EXCL` |
//! | rename | rename | `RNFR`, `RNTO` | `RENAME` |
//! | restore | hard link, then remove | `RNFR`, `RNTO` where the name is free | `RENAME` |
//! | delete | remove | `DELE` | `REMOVE` |
//!
//! On Windows a drop directory's turn is the dropped file itself, opened to
//! write and sharing no writer: one open where a new file took four changes
//! to the directory (ADR-0024, amendment 2026-10-21). Nothing is written
//! through it, the rename goes through it, and it ends with its handle.

use crate::error::Result;

/// A name the listing found, with its stamp where the far end gives one.
pub type Listed<S> = (String, Option<S>);

/// What became of a claimed artifact asked to go back under its dropped
/// name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Restore {
    /// It lies under its dropped name again.
    Back,
    /// It stays claimed, and why: a newer one lies under the dropped name,
    /// or the far end offers no way back that never replaces one.
    Kept(String),
    /// Nothing lies under the claimed name: there is nothing to return.
    Gone,
}

/// One location of the file family, as a claim needs it.
pub trait ClaimSite {
    /// What says an artifact is unchanged: a length and a modification
    /// time, as the far end gives them.
    type Stamp: Clone + Eq + Send + 'static;

    /// Every artifact lying at the location, each with its stamp: files,
    /// never directories. Turns, claimed names and sent files not yet in
    /// place are left out by the caller ([`super::may_arrive`]), and a
    /// starting node finds among them what it left
    /// ([`super::Claimant::recover`]).
    ///
    /// # Errors
    /// Where the location could not be listed.
    fn listed(&self) -> Result<Vec<Listed<Self::Stamp>>>;

    /// Every turn lying at the location, by name — a file or a directory,
    /// as the far end takes one: what a starting node looks through for
    /// the turns it left.
    ///
    /// # Errors
    /// Where the location could not be listed.
    fn turns(&self) -> Result<Vec<String>>;

    /// The stamp of `name` as it lies now; `None` where it is gone, or the
    /// far end says nothing of it.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn stamp(&self, name: &str) -> Result<Option<Self::Stamp>>;

    /// Whether a writer still holds `name` open, where the far end can
    /// tell: `Some(false)` while one writes it, `Some(true)` once none
    /// does — finished, without waiting for a second listing — and `None`
    /// where it cannot tell, so the stability check decides (ADR-0024,
    /// amendment 2026-10-20). Nothing tells, unless a technology says.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn finished(&self, name: &str) -> Result<Option<bool>> {
        let _ = name;
        Ok(None)
    }

    /// Create `turn` only where nothing of that name exists, recording
    /// `claimant` as its holder: `false` where it exists already.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn take_turn(&self, turn: &str, claimant: &str) -> Result<bool>;

    /// Who holds `turn`; `None` where it is gone or names nobody.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn holder(&self, turn: &str) -> Result<Option<String>>;

    /// Remove `turn`, which `claimant` holds.
    ///
    /// # Errors
    /// Where the far end refused.
    fn drop_turn(&self, turn: &str, claimant: &str) -> Result<()>;

    /// Rename `from` to `to`, a name nothing has: `false` where `from` is
    /// gone or the far end refused it, as it refuses a file a producer
    /// holds.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn rename(&self, from: &str, to: &str) -> Result<bool>;

    /// `claimed` back under `dropped`, never over an artifact dropped there
    /// since.
    ///
    /// # Errors
    /// Where the far end could not be asked.
    fn restore(&self, claimed: &str, dropped: &str) -> Result<Restore>;

    /// Delete `name`; one already gone is deleted.
    ///
    /// # Errors
    /// Where the far end refused.
    fn delete(&self, name: &str) -> Result<()>;
}
