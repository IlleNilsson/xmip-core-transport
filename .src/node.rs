//! The node a transport is built on, named by its location.
//!
//! The owner's *Option A*, 2026-10-03: **the runtime gives every transport
//! its node's identity once, as it builds it** — through
//! [`Configured::on_node`](crate::Configured::on_node), placed once here so
//! any technology can use it, never a node argument on each technology's
//! constructor. What a far end records of who holds what — a claimed file's
//! name (ADR-0024, amendment 2026-09-26), a lease's owner — names the node by
//! it, and a node that starts finds by it what it held before it stopped.
//!
//! The value is the node's location, `xmip:///<cluster>/node/<name>`, as the
//! runtime writes it in one place (ADR-0027 clause 4, ADR-0053). A transport
//! reads nothing out of it: it is compared whole, and written where the far
//! end keeps a name.

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
