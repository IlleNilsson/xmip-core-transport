//! Who claims, and the names a claim writes at the far end.
//!
//! ADR-0024, amendment 2026-09-26: asked to settle how a file is claimed,
//! the owner chose *an atomic rename*. A node claims a file by renaming it
//! to a name that says which node took it and when; whichever rename
//! succeeds owns the file. The amendment of 2026-10-13 extends it to every
//! technology of the file family ([`ClaimSite`]).
//!
//! **The claimed name** is the dropped name, the mark `.xmip-claim.`, the
//! node's location percent-encoded so that a file name carries it on every
//! far end (`net::percent`), and the time of the claim in nanoseconds since
//! the Unix epoch:
//!
//! ```text
//! order.edi.xmip-claim.xmip%3A%2F%2F%2F<cluster>%2Fnode%2F<node>.1790000000123456789
//! ```
//!
//! **Two renames of one file are made one at a time.** A rename is atomic,
//! and two of the same file to two names are not exclusive: Windows renames
//! through a handle opened on the file first, so two nodes that both open it
//! both rename it, the second moving it from under the first (found by the
//! file transport's test of two receivers, 2026-10-03); an FTP server may
//! answer `RNFR` to both. So the rename is made holding the file's turn,
//! `order.edi.xmip-claim`, created only where nothing of that name exists —
//! the one exclusive step every far end of the family offers — recording
//! its holder, and removed once the rename is done — on Windows a drop
//! directory's turn is the dropped file held open, sharing no writer
//! (`site.rs`). A claimant that finds the turn taken leaves the file to the
//! next listing.
//!
//! A name carrying the mark, a turn's name, or a sent file's not yet in
//! place (`unfinished.rs`), is never an arrival ([`may_arrive`]).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;
use crate::node::NodeLocation;

use super::site::ClaimSite;
use super::unfinished::is_unfinished;

/// What a claimed name carries between the dropped name and the claimant.
pub const MARK: &str = ".xmip-claim.";

/// What a file's turn adds to its dropped name.
pub const TURN: &str = ".xmip-claim";

/// Who claims: the node, as a claimed name records it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claimant(String);

impl Claimant {
    /// The node at `node`, every byte outside letters, digits, `-` and `_`
    /// percent-encoded: no `.` and no `/`, so a claimed name reads back in
    /// one way and names no directory.
    #[must_use]
    pub fn of(node: &NodeLocation) -> Self {
        Self(net::percent::encode_keeping(node.as_str(), |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
        }))
    }

    /// A transport not yet given its node — a loopback, a test: this
    /// process, so that two processes never mistake one another's claims.
    /// Nothing returns its claims; a node does that as it starts.
    #[must_use]
    pub fn process() -> Self {
        Self(format!("process-{}", std::process::id()))
    }

    /// The claimant as a name carries it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Claim `dropped` at `site` where `still` says, holding its turn, that
    /// it is the artifact that was found: renamed to its claimed name, which
    /// is handed back. `None` where it is not to be had now — its turn is
    /// another's, another claimant renamed it first, or the far end refused
    /// — so it is left to the next listing.
    ///
    /// # Errors
    /// Where the far end could not be asked. A turn taken is removed first.
    pub fn claim<P: ClaimSite + ?Sized>(
        &self,
        site: &P,
        dropped: &str,
        still: impl FnOnce() -> Result<bool>,
    ) -> Result<Option<String>> {
        let turn = turn_of(dropped);
        if !site.take_turn(&turn, &self.0)? {
            return Ok(None);
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let claimed = format!("{dropped}{MARK}{}.{nanos}", self.0);
        let renamed = match still() {
            Ok(true) => site.rename(dropped, &claimed),
            other => other,
        };
        let dropped_turn = site.drop_turn(&turn, &self.0);
        let renamed = renamed?;
        dropped_turn?;
        Ok(renamed.then_some(claimed))
    }

    /// Every artifact this claimant left claimed at `site`, to be returned
    /// under the name it was dropped with; every turn it left, and every
    /// sent file its sends left unfinished, removed
    /// ([`Claimant::remove_unfinished`]): what a node does as it starts,
    /// since whatever it held or was writing before, nothing holds now.
    ///
    /// # Errors
    /// Where `site` could not be listed, or a turn or a sent file not
    /// removed.
    pub fn recover<P: ClaimSite + ?Sized>(&self, site: &P) -> Result<Vec<String>> {
        let (left, others): (Vec<String>, Vec<String>) = site
            .listed()?
            .into_iter()
            .map(|(name, _)| name)
            .partition(|name| claim_of(name).is_some_and(|claim| claim.claimant == self.0));
        self.remove_unfinished(site, others)?;
        for turn in site.turns()? {
            if site.holder(&turn)?.as_deref() == Some(self.0.as_str()) {
                site.drop_turn(&turn, &self.0)?;
            }
        }
        Ok(left)
    }
}

/// A claimed name, read back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim<'a> {
    /// The name the artifact was dropped with.
    pub dropped: &'a str,
    /// Who claimed it.
    pub claimant: &'a str,
}

/// The turn of the artifact dropped as `dropped`.
#[must_use]
pub fn turn_of(dropped: &str) -> String {
    format!("{dropped}{TURN}")
}

/// The claim `name` records; nothing where it records none.
#[must_use]
pub fn claim_of(name: &str) -> Option<Claim<'_>> {
    let at = name.rfind(MARK)?;
    let (claimant, nanos) = name[at + MARK.len()..].split_once('.')?;
    let read = at > 0
        && !claimant.is_empty()
        && !nanos.is_empty()
        && nanos.bytes().all(|byte| byte.is_ascii_digit());
    read.then(|| Claim {
        dropped: &name[..at],
        claimant,
    })
}

/// Whether `name` is an artifact's turn.
#[must_use]
pub fn is_turn(name: &str) -> bool {
    name.len() > TURN.len() && name.ends_with(TURN)
}

/// Whether `name` records a claim or is an artifact's turn: such a name is
/// never an arrival.
#[must_use]
pub fn is_claimed(name: &str) -> bool {
    is_turn(name) || claim_of(name).is_some()
}

/// Whether `name` may be an arrival: it records no claim, is no turn, and
/// is no sent file not yet in place ([`super::is_unfinished`]).
#[must_use]
pub fn may_arrive(name: &str) -> bool {
    !is_claimed(name) && !is_unfinished(name)
}
