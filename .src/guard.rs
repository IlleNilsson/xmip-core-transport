//! What a node guards its receives with: TLS terminated at the transport,
//! presenting the node's `receive` certificate and, where the node was
//! given anchors for it, verifying the client's (ADR-0033 clause 1, steps 1
//! and 2 of its build order).
//!
//! The node hands it to every transport it builds, once, as it builds it
//! ([`Configured::guarded_by`](crate::Configured::guarded_by)); a Location's
//! own settings say whether its transport terminates TLS, and the node's
//! configuration where the certificate comes from (`[provision.receive]`,
//! ADR-0034). A technology never reads a certificate of its own.
//!
//! What the handshake proved is written on the arrival under the names the
//! identity gates read (`context::property`): the peer certificate's
//! subject, issuer and fingerprint, and the handshake's word that it
//! verified the chain, so the first gate presents `mutual-tls` and the
//! second records what the transport proved (ADR-0033, amendment
//! 2026-09-18).
//!
//! **The handshake is read under its own bound.** A Location's own wait —
//! how long a receive waits on a read — bounds the content; the handshake
//! is the guard's, and a client whose handshake messages come a little
//! apart is not cut off by a wait meant for content. Each handshake read
//! waits at most [`HANDSHAKE`], or what the node's `[tuning]
//! receive_handshake` says, and the Location's wait is put back before a
//! byte of content is read (the millisecond rule: a longer receive wait is
//! never the fix).

use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use context::property::{
    TLS_HANDSHAKE_VERIFIED, TLS_PEER_FINGERPRINT, TLS_PEER_ISSUER, TLS_PEER_SUBJECT,
    TLS_PEER_VERIFIED,
};

use crate::error::{Result, TransportError, classify};

/// The most each read of a handshake waits on the client, where the node's
/// `[tuning] receive_handshake` does not say: five seconds, as long as a
/// node's Event link waits on its peer's handshake — the assistant's
/// drafting, 2026-10-20, for the owner to overrule.
pub const HANDSHAKE: Duration = Duration::from_secs(5);

/// The server side of TLS a node's receives are guarded by.
#[derive(Clone, Debug)]
pub struct Guard {
    config: Arc<tls::ServerConfig>,
    /// The most each handshake read waits on the client.
    handshake: Duration,
}

impl Guard {
    /// Guard by `config`: a server configuration presenting the node's
    /// `receive` certificate, verifying clients where it was built to. Its
    /// handshake reads wait [`HANDSHAKE`].
    #[must_use]
    pub const fn new(config: Arc<tls::ServerConfig>) -> Self {
        Self {
            config,
            handshake: HANDSHAKE,
        }
    }

    /// Each handshake read waiting at most `handshake` on the client.
    #[must_use]
    pub const fn handshaking_within(mut self, handshake: Duration) -> Self {
        self.handshake = handshake;
        self
    }

    /// The most each handshake read waits on the client.
    #[must_use]
    pub const fn handshake(&self) -> Duration {
        self.handshake
    }

    /// Answer `tcp` in TLS and finish its handshake before a byte of
    /// content is read: the guarded connection, and what the handshake
    /// proved of the client, under the names the identity gates read —
    /// nothing where it presented no certificate, which a server that
    /// verifies clients has refused already. The handshake is read under
    /// [`Guard::handshake`], and the waits `tcp` came with are put back
    /// for its content.
    ///
    /// # Errors
    ///
    /// Where the handshake fails: a client with no certificate or one the
    /// anchors do not take, one that is not TLS, or one that stopped. Each
    /// is that client's alone, so a Location goes on to the next: the
    /// failure is retryable.
    pub fn answer(&self, tcp: TcpStream) -> Result<(tls::GuardedServer, Vec<(String, String)>)> {
        let waits = |at: &str, e: &std::io::Error| classify(at, e);
        let read = tcp
            .read_timeout()
            .map_err(|e| waits("reading the wait", &e))?;
        let write = tcp
            .write_timeout()
            .map_err(|e| waits("reading the wait", &e))?;
        let bound = Some(self.handshake);
        tcp.set_read_timeout(bound)
            .and_then(|()| tcp.set_write_timeout(bound))
            .map_err(|e| waits("bounding the handshake", &e))?;
        let (guarded, peer) = tls::peer::answered(tcp, Arc::clone(&self.config))
            .map_err(|refused| TransportError::retryable(refused.message))?;
        guarded
            .sock
            .set_read_timeout(read)
            .and_then(|()| guarded.sock.set_write_timeout(write))
            .map_err(|e| waits("putting the Location's wait back", &e))?;
        let observed = peer.map(|peer| observed(&peer)).unwrap_or_default();
        Ok((guarded, observed))
    }
}

/// What the handshake proved of `peer`, under the identity gates' names.
fn observed(peer: &tls::Peer) -> Vec<(String, String)> {
    [
        (TLS_PEER_SUBJECT, peer.subject.as_str()),
        (TLS_PEER_ISSUER, peer.issuer.as_str()),
        (TLS_PEER_FINGERPRINT, peer.fingerprint.as_str()),
        (TLS_PEER_VERIFIED, TLS_HANDSHAKE_VERIFIED),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verified_client_is_written_under_the_names_the_gates_read() {
        let peer = tls::Peer {
            subject: "CN=party-x.example".to_string(),
            issuer: "CN=Party CA".to_string(),
            fingerprint: "SHA256:ab".to_string(),
        };
        let written = observed(&peer);

        assert!(written.contains(&(TLS_PEER_SUBJECT.into(), "CN=party-x.example".into())));
        assert!(written.contains(&(TLS_PEER_VERIFIED.into(), TLS_HANDSHAKE_VERIFIED.into())));
        assert_eq!(written.len(), 4);
    }
}
