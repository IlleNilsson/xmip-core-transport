//! The module's timing program (`doc/governance/rust-style.md` section
//! 5c): the transport machinery every technology builds on, held to how
//! long Xmip takes — the millisecond rule (`CONTRIBUTING.md`, *Rust is here
//! for one reason*). A plain `cargo test` leaves it out; the landing runs
//! it after every other test of the module, alone, one test at a time
//! (`cargo test --test measured --all-features -- --test-threads=1`), so
//! what is measured is Xmip's work and not its neighbors':
//!
//! - `socket.rs`: a connection is accepted as it lands, not after a nap;
//! - `serving.rs`: a connection freed while a receive waits is waited on at
//!   once, and exchanges on a kept connection take under a millisecond each;
//! - `pool.rs`: a thousand exchanges on one key, one session, a millisecond
//!   each at most;
//! - `claim.rs`: a starting node's removal of what its sends left costs no
//!   more than a millisecond beyond the far end's own work.

mod claim;
mod pool;
mod serving;
mod socket;
