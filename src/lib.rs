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
//!   transport.rs        the Transport trait
//!   direction.rs        which directions an implementation supports
//!   arrived.rs          one Stream, and where it came from
//!   error.rs            failure, and whether saying it again would help
//!   claim.rs            one holder of a collidable artefact at a time
//!
//! shared machinery
//!   wire.rs             reading line-oriented protocols, used by http and smtp
//!   technology.rs       what each technology is built on, and what reuses it
//!
//! one protocol each
//!   file.rs  tcp.rs  udp.rs  http/  smtp/
//! ```
//!
//! *`technology.rs` arrived here on 2026-08-26 from the root's
//! `transport_technology.rs` and was never declared, so it has never compiled.
//! Declared 2026-08-27 during this split — which is also the only reason serde
//! is a dependency.*
//!
//! **`architecture.toml` declares 84 transports and five are implemented.** Each
//! is declared as its own repository — `xmip-core-transport-kafka` and eighty
//! siblings. The five here are separated along that line so that lifting one out
//! is a move rather than a rewrite: nothing above the protocol files knows which
//! protocols exist, and no protocol file knows about another.

pub mod arrived;
pub mod claim;
pub mod direction;
pub mod error;
pub mod file;
pub mod http;
pub mod protocol;
pub mod smtp;
pub mod tcp;
pub mod technology;
pub mod udp;
pub mod wire;

pub use arrived::Arrived;
pub use claim::{Artefact, Claimed, NoNativeClaim, ResourceClaim};
pub use direction::Directions;
pub use error::{Result, TransportError};
pub use file::FileTransport;
pub use http::HttpTransport;
pub use protocol::Transport;
pub use smtp::SmtpTransport;
pub use tcp::TcpTransport;
pub use udp::UdpTransport;
