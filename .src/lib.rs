#![forbid(unsafe_code)]

//! The Transport capability: one protocol, both directions.
//!
//! Direction-neutral, per ADR-0010. One protocol, one implementation, and the
//! artifact decides whether it receives or sends — HTTP is the same protocol
//! whether Xmip is listening or calling, which is why the receive and send sides
//! are two methods on one trait rather than two repositories.
//!
//! Shaped to mirror `XmipTransportVtable` in `include/xmip_module.h` so that
//! moving an implementation across the C boundary later is mechanical rather
//! than a redesign.
//!
//! # The shape of this crate
//!
//! ```text
//! the contract          what every protocol implements, and nothing protocol-specific
//!   protocol.rs         the Transport trait
//!   direction.rs        which directions an implementation supports
//!   arrived.rs          one Stream, where it came from, its body as a reader; the next
//!                       or the one arrival
//!   arrivals.rs         whether a technology's arrivals are told in order or each
//!                       on its own, and why
//!   acknowledgement.rs  what the far end is told once the receive cycle ended: the
//!                       verdict, or why the protocol could not defer (at-most-once)
//!   taken.rs            what a far end took whole, already answered
//!   together.rs         arrivals a far end settles together, after the last verdict
//!   body.rs             a body opened by its first read, or pulled a chunk at a time
//!   contiguous.rs       a place in an ordered log that only a contiguous acceptance moves
//!   listed.rs           a receive from an object store: its XML listing scanned, each
//!                       fetched when read, deleted once accepted, left and remembered
//!                       once refused
//!   refused.rs          what a Location refused and left where it lies, each by its name
//!                       and stamp, left out of a listing while it lies as it was refused
//!   answer.rs           the answer a peer waits for after the cycle: on its connection,
//!                       busy until given and shut when let go, or as a datagram
//!   error.rs            failure, and whether saying it again would help
//!   claim.rs            one holder of a collidable artefact at a time
//!   configured.rs       the settings a technology declares, and the one way a
//!                       Location's address and settings build it (ADR-0064)
//!   node.rs             the node a transport is built on, by its location, given
//!                       once as the runtime builds it
//!   loopback.rs         a transport that is both ends of one exchange, with its
//!                       ceiling and refusals; what the Playground drives (ADR-0051);
//!                       both ends on two threads, and the poke that releases a far end
//!
//! the far ends          what a technology's loopback stands up (ADR-0051)
//!   listening.rs        a bound TCP listener waiting for its one connection
//!   bound.rs            a bound UDP socket waiting for its one exchange
//!   held.rs             a far end in this process: a bus device, a radio's server, a pipe
//!   standing.rs         in-process sessions stood up by address until taken
//!
//! the near ends         what a Receive Location keeps between its receives
//!   kept.rs             a listener or socket bound on the first receive and kept
//!   serving.rs          a kept listener and the connections its peers keep open on
//!                       it, the next exchange taken from whichever speaks first
//!
//! shared machinery      what two technologies both need lives here (ADR-0044)
//!   socket.rs           binding, accepting (waiting on readiness), connecting and
//!                       splitting sockets; multicast
//!   sender.rs           the datagram socket a UDP technology sends from, bound once
//!                       per address family and kept
//!   pool.rs             the sessions a transport keeps between sends and receives,
//!                       by address: opened and logged in once, reused, replaced
//!                       when broken; a kept subscription drained until quiet; a
//!                       delivery answered on the session it came on, or not at all
//!   login.rs            a user and the password it logs in with
//!   stuffed.rs          the dot-stuffed block mail speaks: smtp, pop3
//!   sql.rs              a send target read, the one INSERT written with quoted identifiers
//!                       and taken apart, the verb, the fixed table a SQL far end serves, and
//!                       the declared payload column: binary, or text in its Unicode form
//!     accept.rs         the rows a query read as arrivals, and the `accept` statement that
//!                       consumes each after its cycle, the row's name bound in
//!
//! The byte cursor and writer, hex, the CRCs and the varint every binary
//! technology frames with are encoding primitives, not transport: they live in
//! `xmip-core-library-codec`, which each technology depends on itself. They sat
//! here as `cursor.rs`, `crc.rs` and `hex.rs` from 2026-09-14 until 2026-09-24.
//!
//! test support          compiled for tests only, `test-support` from a dev-dependency
//!   payload.rs          the edge and sized payloads a loopback round is exercised with
//!   latency.rs          a round's median, p99 and worst, and a plain loopback TCP
//!                       wake beside it: near real time, apart from load
//! ```
//!
//! **Nothing here names a protocol.** Each transport technology is its own
//! repository (ADR-0010 decision 3), mounted directly under this one — `file`,
//! `tcp`, `udp`, `http`, `smtp`, `websocket` today — and depends on this crate,
//! never the reverse. The six lived in `src/` from 2026-08-27 until 2026-09-07,
//! kept apart so that lifting them out was a move rather than a rewrite; it was.

pub mod acknowledgement;
pub mod answer;
pub mod arrivals;
pub mod arrived;
pub mod body;
pub mod bound;
pub mod claim;
pub mod configured;
pub mod contiguous;
pub mod direction;
pub mod error;
pub mod held;
pub mod kept;
#[cfg(any(test, feature = "test-support"))]
pub mod latency;
pub mod line;
pub mod listed;
pub mod listening;
pub mod login;
pub mod loopback;
pub mod node;
#[cfg(any(test, feature = "test-support"))]
pub mod payload;
pub mod pool;
pub mod protocol;
pub mod refused;
pub mod sender;
pub mod serving;
pub mod socket;
pub mod sql;
pub mod standing;
pub mod stuffed;
pub mod taken;
pub mod together;

pub use acknowledgement::{Acknowledgement, Refusal, Verdict};
pub use arrivals::Arrivals;
pub use arrived::Arrived;
pub use claim::{Artefact, Claimed, NoNativeClaim, ResourceClaim};
pub use configured::Configured;
pub use direction::Directions;
pub use error::{Result, TransportError};
pub use listening::{Accepting, Listening};
pub use login::Login;
pub use loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback, UNBLOCK_TIMEOUT};
pub use node::NodeLocation;
pub use pool::{Pool, Pooled};
pub use protocol::Transport;
pub use refused::Refused;
pub use taken::Taken;

/// What one TCP segment carries over Ethernet: its 1500-byte MTU less the
/// IP and TCP headers' 40. TCP over IP is the major transport, so the
/// Ledger writes a Stream in a whole number of them (the owner,
/// 2026-10-03: *sized according to IP/TCP chunks with a multiple*). A
/// transport whose own unit serves better may declare one when that is
/// useful; none does yet.
pub const TCP_SEGMENT: usize = 1460;
