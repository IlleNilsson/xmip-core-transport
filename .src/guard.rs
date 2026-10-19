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

use std::net::TcpStream;
use std::sync::Arc;

use context::property::{
    TLS_HANDSHAKE_VERIFIED, TLS_PEER_FINGERPRINT, TLS_PEER_ISSUER, TLS_PEER_SUBJECT,
    TLS_PEER_VERIFIED,
};

use crate::error::{Result, TransportError};

/// The server side of TLS a node's receives are guarded by.
#[derive(Clone, Debug)]
pub struct Guard {
    config: Arc<tls::ServerConfig>,
}

impl Guard {
    /// Guard by `config`: a server configuration presenting the node's
    /// `receive` certificate, verifying clients where it was built to.
    #[must_use]
    pub const fn new(config: Arc<tls::ServerConfig>) -> Self {
        Self { config }
    }

    /// Answer `tcp` in TLS and finish its handshake before a byte of
    /// content is read: the guarded connection, and what the handshake
    /// proved of the client, under the names the identity gates read —
    /// nothing where it presented no certificate, which a server that
    /// verifies clients has refused already.
    ///
    /// # Errors
    ///
    /// Where the handshake fails: a client with no certificate or one the
    /// anchors do not take, one that is not TLS, or one that stopped. Each
    /// is that client's alone, so a Location goes on to the next: the
    /// failure is retryable.
    pub fn answer(&self, tcp: TcpStream) -> Result<(tls::GuardedServer, Vec<(String, String)>)> {
        let (guarded, peer) = tls::peer::answered(tcp, Arc::clone(&self.config))
            .map_err(|refused| TransportError::retryable(refused.message))?;
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
