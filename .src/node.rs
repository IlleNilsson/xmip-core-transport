//! The node a transport is built on: its location, and which start of it
//! the transport serves.
//!
//! The owner's *Option A*, 2026-10-03: **the runtime gives every transport
//! its node's identity once, as it builds it** — through
//! [`Configured::on_node`](crate::Configured::on_node), placed once here so
//! any technology can use it, never a node argument on each technology's
//! constructor. What a far end records of who holds what — a claimed file's
//! name (ADR-0024, amendment 2026-09-26), an object's claim, a lease's owner
//! — names the node by it, and a node that starts finds by it what it held
//! before it stopped.
//!
//! The location is the node's, `xmip:///<cluster>/node/<name>`, as the
//! runtime writes it in one place (ADR-0027 clause 4, ADR-0053). **The
//! start** is which start of the node this is (ADR-0024, amendment
//! 2026-11-22; the owner, *Do them all*): the number its membership row is
//! keyed by, drawn new as the node starts (ADR-0056, amendment 2026-11-13),
//! the same for every Location of that start, so a Location built while
//! another of its start receives tells what that one holds from what an
//! earlier start left. A transport reads nothing out of either: each is
//! compared whole, and written where the far end keeps a name.

use std::fmt;

/// The location of the node a transport serves.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeLocation(String);

impl NodeLocation {
    /// The node at `location`, as the runtime wrote it.
    #[must_use]
    pub fn new(location: impl Into<String>) -> Self {
        Self(location.into())
    }

    /// The location, whole.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One start of the node a transport serves: what the runtime hands every
/// Location's transport of that start ([`Configured::on_node`](crate::Configured::on_node)).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NodeStart {
    location: NodeLocation,
    start: u128,
}

impl NodeStart {
    /// The start `start` of the node at `location`, as the runtime drew it.
    #[must_use]
    pub fn new(location: NodeLocation, start: u128) -> Self {
        Self { location, start }
    }

    /// A start of the node at `location` after every one this process made
    /// so far: what a test builds as its node starting again, where the
    /// runtime hands the start it drew.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn next(location: NodeLocation) -> Self {
        static STARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let start = STARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self::new(location, u128::from(start))
    }

    /// The node's location.
    #[must_use]
    pub fn location(&self) -> &NodeLocation {
        &self.location
    }

    /// Which start of the node this is.
    #[must_use]
    pub fn start(&self) -> u128 {
        self.start
    }
}
