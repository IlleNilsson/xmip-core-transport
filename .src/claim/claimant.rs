//! Who claims, and the names a claim writes at the far end.
//!
//! ADR-0024, amendment 2026-09-26: asked to settle how a file is claimed,
//! the owner chose *an atomic rename*. A node claims a file by renaming it
//! to a name that says which node took it and when; whichever rename
//! succeeds owns the file. The amendment of 2026-10-13 extends it to every
//! technology of the file family ([`ClaimSite`]).
//!
//! **The claimed name** is the dropped name, the mark `.xmip-claim.`, the
//! claimant — the node's location percent-encoded so that a file name
//! carries it on every far end (`codec::percent`), `~` and which start of
//! the node claimed, in 32 hexadecimal digits (`crate::node::NodeStart`) —
//! and the time of the claim in nanoseconds since the Unix epoch:
//!
//! ```text
//! order.edi.xmip-claim.xmip%3A%2F%2F%2F<cluster>%2Fnode%2F<node>~<start>.1790000000123456789
//! ```
//!
//! **A start lets go of what its node's earlier starts left, and of
//! nothing its own start holds** (ADR-0024, amendment 2026-11-22; the
//! owner, *Do them all*): a Location built while another of the same start
//! receives over the same far end leaves that one's claims, turns and sent
//! files ([`Claimant::is_earlier_start`]). The encoding keeps only letters,
//! digits, `-` and `_`, so `~` parts the two in one way; a claimant with
//! none, as one written before 2026-11-22, is its node alone, and reads as
//! an earlier start of it.
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

use xcore::{Clock, SystemClock};

use crate::error::Result;
use crate::node::NodeStart;

use super::site::ClaimSite;
use super::unfinished::is_unfinished;

/// What a claimed name carries between the dropped name and the claimant.
pub const MARK: &str = ".xmip-claim.";

/// What a file's turn adds to its dropped name.
pub const TURN: &str = ".xmip-claim";

/// What a claimant carries between its node and which start of it claims.
const START: char = '~';

/// Who claims: the node and which start of it, as a claimed name records
/// them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claimant(String);

impl Claimant {
    /// The start `node`: its location, every byte outside letters, digits,
    /// `-` and `_` percent-encoded, `~` and its start in 32 hexadecimal
    /// digits — no `.` and no `/`, so a claimed name reads back in one way
    /// and names no directory.
    #[must_use]
    pub fn of(node: &NodeStart) -> Self {
        let location = codec::percent::encode_keeping(node.location().as_str(), |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
        });
        Self(format!("{location}{START}{:032x}", node.start()))
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

    /// Whether `recorded`, the claimant a claim, a turn or a sent file
    /// records, is an earlier start of this claimant's node: the same node,
    /// and not this start, since a node runs one start at a time. One that
    /// records no start, written before a claim carried it, is one. What
    /// another node, or this start through any of its Locations, holds is
    /// not.
    #[must_use]
    pub fn is_earlier_start(&self, recorded: &str) -> bool {
        recorded != self.0 && node_of(recorded) == node_of(&self.0)
    }

    /// The name a claim of `dropped` by this claimant records, now: the
    /// dropped name, the mark, the claimant and the time in nanoseconds
    /// since the Unix epoch. A file family's claimed artifact lies under
    /// it; an object store's claim holds it (`crate::listed`).
    #[must_use]
    pub fn claimed(&self, dropped: &str) -> String {
        let nanos = SystemClock.unix_timestamp_nanos().max(0);
        format!("{dropped}{MARK}{}.{nanos}", self.0)
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
        let claimed = self.claimed(dropped);
        let renamed = match still() {
            Ok(true) => site.rename(dropped, &claimed),
            other => other,
        };
        let dropped_turn = site.drop_turn(&turn, &self.0);
        let renamed = renamed?;
        dropped_turn?;
        Ok(renamed.then_some(claimed))
    }

    /// Every artifact an earlier start of this claimant's node left claimed
    /// at `site`, to be returned under the name it was dropped with; every
    /// turn such a start left, and every sent file its sends left
    /// unfinished, removed ([`Claimant::remove_unfinished`]): what a node's
    /// Location does as it starts, since whatever an earlier start held or
    /// was writing, nothing holds now. What this start holds through
    /// another Location is left ([`Claimant::is_earlier_start`]).
    ///
    /// # Errors
    /// Where `site` could not be listed, or a turn or a sent file not
    /// removed.
    pub fn recover<P: ClaimSite + ?Sized>(&self, site: &P) -> Result<Vec<String>> {
        let (left, others): (Vec<String>, Vec<String>) = site
            .listed()?
            .into_iter()
            .map(|(name, _)| name)
            .partition(|name| {
                claim_of(name).is_some_and(|claim| self.is_earlier_start(claim.claimant))
            });
        self.remove_unfinished(site, others)?;
        for turn in site.turns()? {
            if let Some(holder) = site.holder(&turn)?
                && self.is_earlier_start(&holder)
            {
                site.drop_turn(&turn, &holder)?;
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

/// The node a claimant `recorded` names: what comes before its start, or
/// all of it where it records none.
fn node_of(recorded: &str) -> &str {
    recorded
        .split_once(START)
        .map_or(recorded, |(node, _)| node)
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
