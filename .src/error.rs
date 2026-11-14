//! Failure, and the one fact resilience needs from it.

use std::io;

xcore::declare_retryable_error!(
    /// A transport failure.
    ///
    /// `retryable` mirrors `XMIP_IS_RETRYABLE`: it is a property **of the
    /// failure**, not of the call site, so `xmip-core-resilience` can decide
    /// what to do without knowing which implementation produced it. The shape
    /// is `xcore::Failure`'s, declared once in `xmip-core`; this crate names
    /// it so its dependencies' errors convert into it, and it converts into
    /// `xcore::Failure` — the failure the guards judge — retryable as it was.
    TransportError
);

pub type Result<T> = std::result::Result<T, TransportError>;

/// Classify an I/O failure.
///
/// A blip is retryable; a missing file or a refused permission is not. Getting
/// this wrong is how a platform either gives up too early or retries forever.
#[must_use]
pub fn classify(context: &str, error: &io::Error) -> TransportError {
    TransportError {
        message: format!("{context}: {}", said(error)),
        retryable: passes(error.kind()) || exhausted(error),
    }
}

/// Whether a failure of this kind may pass on a second attempt: a blip, a
/// timeout, a reset, a refusal, a port not yet free, a peer gone — its
/// pipe broken, its socket no longer connected.
///
/// A peer gone is retryable from 2026-11-13: an answer written on the
/// connection of a caller that hung up before its verdict failed with a
/// broken pipe (EPIPE after the peer's reset, `WSAESHUTDOWN`) or not
/// connected, and as a permanent failure it stopped the Receive Location
/// for every other caller, where the caller only sends again.
const fn passes(kind: io::ErrorKind) -> bool {
    use io::ErrorKind::{
        AddrInUse, AddrNotAvailable, BrokenPipe, ConnectionAborted, ConnectionRefused,
        ConnectionReset, Interrupted, NotConnected, TimedOut, WouldBlock,
    };

    matches!(
        kind,
        Interrupted
            | WouldBlock
            | TimedOut
            | ConnectionReset
            | ConnectionAborted
            | ConnectionRefused
            | BrokenPipe
            | NotConnected
            | AddrInUse
            | AddrNotAvailable
    )
}

/// Whether the operating system ran out of what a socket takes — the
/// process's descriptors (`EMFILE`, `WSAEMFILE`), the system's (`ENFILE`),
/// its network buffers (`ENOBUFS`, `WSAENOBUFS`). They come back as other
/// sockets close, so asking again may pass; std names no kind for them, so
/// they are read by their number. Until 2026-11-13 they were permanent, and
/// one accept that met them stopped the Receive Location for good.
fn exhausted(error: &io::Error) -> bool {
    use rustix::io::Errno;

    let Some(errno) = Errno::from_io_error(error) else {
        return false;
    };
    #[cfg(not(windows))]
    if errno == Errno::NFILE {
        return true;
    }
    errno == Errno::MFILE || errno == Errno::NOBUFS
}

/// What an operating system said, in the estate's words where its own are
/// unhelpful.
///
/// One case so far, and it earned itself: Windows answers a machine with no
/// ephemeral port left with *Only one usage of each socket address
/// (protocol/network address/port) is normally permitted*, which names
/// neither the resource nor the wait. It cost a day to read, because the
/// Essentials exhausted the range and the message read as a bind collision
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

/// A network primitive's failure: a connection's judged by its kind as
/// [`classify`] judges one, anything else — an answer that is not HTTP, a
/// URL that is not one — permanent.
impl From<net::NetError> for TransportError {
    fn from(error: net::NetError) -> Self {
        Self {
            retryable: error.io.is_some_and(passes),
            message: error.message,
        }
    }
}

/// A Location's settings its technology's declaration refused: the
/// configuration's to change, so saying it again will not help.
impl From<xcore::settings::Refused> for TransportError {
    fn from(refused: xcore::settings::Refused) -> Self {
        Self::permanent(refused.to_string())
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
    fn a_malformed_x690_element_is_a_permanent_protocol_error() {
        let read_one = |bytes: &[u8]| -> Result<u8> { Ok(asn1::read(bytes)?.0) };
        let error = read_one(&[0x04, 0x05, 1]).expect_err("past the end");
        assert!(!error.retryable);
        assert!(error.message.contains("says 5 bytes"), "{}", error.message);
    }

    #[test]
    fn a_guard_judges_a_transport_failure_as_retryable_as_it_was() {
        let blip = xcore::Failure::from(TransportError::retryable("reset"));
        let refused = xcore::Failure::from(TransportError::permanent("403"));
        assert!(blip.retryable);
        assert_eq!(blip.message, "reset");
        assert!(!refused.retryable);
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
    fn a_peer_gone_before_its_answer_is_retryable() {
        // Permanent until 2026-11-13: the answer to a caller that hung up
        // before its verdict stopped the Receive Location for every other caller.
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::NotConnected] {
            assert!(
                classify("writing the answer", &io_error(kind)).retryable,
                "{kind}"
            );
        }
    }

    #[test]
    fn a_socket_written_after_its_peer_hung_up_fails_retryable() {
        let (listener, address) = crate::socket::bind_tcp("127.0.0.1:0").expect("bound");
        let peer = std::net::TcpStream::connect(&address).expect("connected");
        let (this, _) = listener.accept().expect("accepted");
        drop(peer);
        // The first write may land before the peer's reset; one after it,
        // or after this side shut its writing, fails as a peer gone.
        this.shutdown(std::net::Shutdown::Write).expect("shut");
        let written = io::Write::write_all(&mut &this, b"the verdict")
            .map_err(|e| classify("writing the answer", &e));
        let failed = written.expect_err("its writing was shut");
        assert!(failed.retryable, "{failed}");
    }

    #[test]
    fn a_machine_out_of_descriptors_or_buffers_is_retryable() {
        use rustix::io::Errno;
        for errno in [Errno::MFILE, Errno::NOBUFS] {
            let met = io::Error::from_raw_os_error(errno.raw_os_error());
            assert!(classify("accepting a connection", &met).retryable, "{met}");
        }
        let other = io::Error::from_raw_os_error(Errno::ACCESS.raw_os_error());
        assert!(!classify("binding", &other).retryable, "{other}");
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
    fn a_network_failure_is_judged_by_its_kind_and_a_malformed_answer_never_retried() {
        let refused =
            net::NetError::from_io("connecting", &io_error(io::ErrorKind::ConnectionRefused));
        let missing = net::NetError::from_io("reading", &io_error(io::ErrorKind::NotFound));

        assert!(TransportError::from(refused).retryable);
        assert!(!TransportError::from(missing).retryable);
        assert!(!TransportError::from(net::NetError::new("not an HTTP answer")).retryable);
    }

    #[test]
    fn a_broken_protocol_is_never_retryable() {
        assert!(!protocol_error("nonsense on the wire").retryable);
    }

    #[test]
    fn the_judgment_is_visible_in_the_message() {
        // An operator reads this in a log without the struct around it.
        assert_eq!(
            TransportError::retryable("the peer hung up").to_string(),
            "the peer hung up (retryable)"
        );
    }
}
