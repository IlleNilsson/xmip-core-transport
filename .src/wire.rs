//! The most a connection is read for.
//!
//! One ceiling for every technology that reads a Stream off a connection:
//! a peer claiming four gigabytes must not get four gigabytes allocated.
//!
//! Nothing here knows which protocol is calling. The head the line-oriented
//! protocols write — lines, then a blank line — was read here until
//! 2026-09-25 and is `net::head` now, beside HTTP's exchange in `net::http`
//! that reads it; the authority a protocol connects to is a URI's, and is
//! read in `xmip-core-library-net` since 2026-09-24.

/// The largest single Stream Xmip will read off one connection.
pub const MAX_BODY: usize = 64 * 1024 * 1024;
