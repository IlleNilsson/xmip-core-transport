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

    /// The same failure, said from where it was met: `"<where>: <message>"`.
    ///
    /// Retryability is the failure's own property and survives the wrapping.
    /// Writing `format!("{error}")` into a new failure instead loses it — a
    /// timeout becomes permanent, and resilience stops retrying what it
    /// should retry — and doubles the judgement in the text, which is how it
    /// was found: a Linux run read *(retryable) (not retryable)* on one line
    /// (2026-09-19).
    #[must_use]
    pub fn at(mut self, where_met: &str) -> Self {
        self.message = format!("{where_met}: {}", self.message);
        self
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
        AddrInUse, AddrNotAvailable, ConnectionAborted, ConnectionRefused, ConnectionReset,
        Interrupted, TimedOut, WouldBlock,
    };

    let retryable = matches!(
        error.kind(),
        Interrupted
            | WouldBlock
            | TimedOut
            | ConnectionReset
            | ConnectionAborted
            | ConnectionRefused
            | AddrInUse
            | AddrNotAvailable
    );

    TransportError {
        message: format!("{context}: {}", said(error)),
        retryable,
    }
}

/// What an operating system said, in the estate's words where its own are
/// unhelpful.
///
/// One case so far, and it earned itself: Windows answers a machine with no
/// ephemeral port left with *Only one usage of each socket address
/// (protocol/network address/port) is normally permitted*, which names
/// neither the resource nor the wait. It cost a day to read, because the
/// Playground exhausted the range and the message read as a bind collision
/// (2026-09-21). A raw TCP Stream is one connection (`xmip-core-transport-tcp`
/// README), so a node sending at volume over unframed TCP spends one local
/// port per Stream and the operating system holds it for minutes afterwards.
fn said(error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::AddrInUse {
        return format!(
            "no local port was free — the machine's ephemeral range is spent \
             and ports return as their close-wait expires ({error})"
        );
    }

    error.to_string()
}

/// A connection that could not be guarded: permanent, because a certificate
/// or a trust store does not fix itself between attempts.
#[cfg(feature = "tls")]
impl From<tls::TlsError> for TransportError {
    fn from(error: tls::TlsError) -> Self {
        Self::permanent(error.message)
    }
}

/// Text that is not the encoding it claims: a peer that broke the protocol.
impl From<codec::CodecError> for TransportError {
    fn from(error: codec::CodecError) -> Self {
        Self::permanent(error.message)
    }
}

/// X.690 that is not what it says it is: a peer that broke the protocol.
impl From<asn1::Asn1Error> for TransportError {
    fn from(error: asn1::Asn1Error) -> Self {
        Self::permanent(error.message)
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
    fn a_machine_out_of_ports_is_retryable_and_says_so_in_words() {
        // It was permanent until 2026-09-21, which is backwards: a spent
        // ephemeral range is the most retryable condition there is, since the
        // ports come back on their own. And Windows' own words for it name
        // neither the resource nor the wait, which cost a day of reading it
        // as a bind collision.
        let met = classify(
            "connecting to the peer",
            &io_error(io::ErrorKind::AddrInUse),
        );

        assert!(met.retryable, "the ports come back");
        assert!(
            met.message.contains("no local port was free"),
            "an operator is told what ran out: {met}"
        );
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
