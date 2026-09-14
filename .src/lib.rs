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
//!   arrived.rs          one Stream, and where it came from
//!   error.rs            failure, and whether saying it again would help
//!   claim.rs            one holder of a collidable artefact at a time
//!   loopback.rs         a transport that is both ends of one exchange, with its
//!                       ceiling and refusals; what the Playground drives (ADR-0051)
//!   listening.rs        the far end of every TCP technology's round: a bound listener
//!
//! shared machinery      what two technologies both need lives here (ADR-0044)
//!   wire.rs             reading line-oriented protocols: http, smtp
//!   socket.rs           binding, accepting, connecting and splitting sockets; multicast
//!   stuffed.rs          the dot-stuffed block mail speaks: smtp, pop3
//!   xml.rs              the flat-XML scan a protocol document is
//!   label.rs            the label-and-pointer form of a DNS name: dns, mdns
//!   ber.rs              X.690 tag-length-value: iec-61850, snmp
//!   sql.rs              the one INSERT, the verb and the fixed table a SQL far end serves
//!   hex.rs              bytes as hex pairs and back
//!   technology.rs       what each technology is built on, and what reuses it
//!
//! test support          compiled for tests only, `test-support` from a dev-dependency
//!   payload.rs          the edge and sized payloads a loopback round is exercised with
//! ```
//!
//! *`technology.rs` arrived here on 2026-08-26 from the root's
//! `transport_technology.rs` and was never declared, so it has never compiled.
//! Declared 2026-08-27 during this split — which is also the only reason serde
//! is a dependency.*
//!
//! **Nothing here names a protocol.** Each transport technology is its own
//! repository (ADR-0010 decision 3), mounted directly under this one — `file`,
//! `tcp`, `udp`, `http`, `smtp`, `websocket` today — and depends on this crate,
//! never the reverse. The six lived in `src/` from 2026-08-27 until 2026-09-07,
//! kept apart so that lifting them out was a move rather than a rewrite; it was.

pub mod arrived;
pub mod ber;
pub mod claim;
pub mod direction;
pub mod error;
pub mod hex;
pub mod label;
pub mod listening;
pub mod loopback;
#[cfg(any(test, feature = "test-support"))]
pub mod payload;
pub mod protocol;
pub mod socket;
pub mod sql;
pub mod stuffed;
pub mod technology;
pub mod wire;
pub mod xml;

pub use arrived::Arrived;
pub use claim::{Artefact, Claimed, NoNativeClaim, ResourceClaim};
pub use direction::Directions;
pub use error::{Result, TransportError};
pub use listening::{Accepting, Listening};
pub use loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
pub use protocol::Transport;
