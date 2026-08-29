//! Failure, and the one fact resilience needs from it.

use std::fmt;
use std::io;

/// A transport failure.
///
/// `retryable` mirrors `XMIP_IS_RETRYABLE`: it is a property **of the failure**,
/// not of the call site, so `xmip-core-resilience` can decide what to do without
/// knowing which implementation produced it.
#[derive(Debug)]
pub struct TransportError {
    pub message: String,
    pub retryable: bool,
}

impl TransportError {
    /// A failure worth trying again. A blip, a timeout, a reset.
    #[must_use]
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    /// A failure that will say the same thing next time.
    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let judgement = if self.retryable {
            "retryable"
        } else {
            "not retryable"
        };

        write!(f, "{} ({judgement})", self.message)
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

/// Classify an I/O failure.
///
/// A blip is retryable; a missing file or a refused permission is not. Getting
/// this wrong is how a platform either gives up too early or retries forever.
#[must_use]
pub fn classify(context: &str, error: &io::Error) -> TransportError {
    use io::ErrorKind::{
        ConnectionAborted, ConnectionRefused, ConnectionReset, Interrupted, TimedOut, WouldBlock,
    };

    let retryable = matches!(
        error.kind(),
        Interrupted | WouldBlock | TimedOut | ConnectionReset | ConnectionAborted | ConnectionRefused
    );

    TransportError {
        message: format!("{context}: {error}"),
        retryable,
    }
}

/// A peer that broke the protocol. Saying it again will not help.
#[must_use]
pub fn protocol_error(message: impl Into<String>) -> TransportError {
    TransportError::permanent(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error(kind: io::ErrorKind) -> io::Error {
        io::Error::new(kind, "under test")
    }

    #[test]
    fn a_missing_file_is_not_retryable() {
        assert!(!classify("reading", &io_error(io::ErrorKind::NotFound)).retryable);
    }

    #[test]
    fn a_refused_connection_is_retryable() {
        assert!(classify("connecting", &io_error(io::ErrorKind::ConnectionRefused)).retryable);
    }

    #[test]
    fn a_broken_protocol_is_never_retryable() {
        assert!(!protocol_error("nonsense on the wire").retryable);
    }

    #[test]
    fn the_judgement_is_visible_in_the_message() {
        // An operator reads this in a log without the struct around it.
        assert_eq!(
            TransportError::retryable("the peer hung up").to_string(),
            "the peer hung up (retryable)"
        );
    }
}
