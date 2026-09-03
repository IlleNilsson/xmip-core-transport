//! Wrapping a connection in TLS, verified against the operating system store.
//!
//! Behind the `tls` feature, which is off by default. Everything here is
//! compiled out otherwise, and `HttpTransport::send` refuses `https://` with a
//! message saying so rather than silently sending in the clear.

use std::net::TcpStream;
use std::sync::Arc;

use crate::error::{Result, protocol_error};

/// A TLS connection ready to read and write.
pub type Guarded = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// Wrap a connection in TLS for one host.
///
/// # Errors
///
/// Where the trust store is empty or unreadable, the host is not a name TLS can
/// use, or the handshake could not be started.
pub fn client(host: &str, tcp: TcpStream) -> Result<Guarded> {
    let config = Arc::new(configure()?);

    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| protocol_error(format!("a server name tls cannot use: {host}")))?;

    let connection = rustls::ClientConnection::new(config, name)
        .map_err(|e| protocol_error(format!("starting the tls session: {e}")))?;

    Ok(rustls::StreamOwned::new(connection, tcp))
}

fn configure() -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let versions = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| protocol_error(format!("selecting tls versions: {e}")))?;

    Ok(versions
        .with_root_certificates(native_roots()?)
        .with_no_client_auth())
}

/// The operating system trust store.
///
/// The native store rather than a bundled root list, because the organisations
/// Xmip is aimed at run internal certificate authorities and expect their own
/// certificates to work without waiting for Xmip to ship a new root bundle.
fn native_roots() -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();

    for certificate in rustls_native_certs::load_native_certs().certs {
        // One unparsable certificate is not a reason to refuse every other
        // certificate in the store.
        let _ = roots.add(certificate);
    }

    if roots.is_empty() {
        return Err(protocol_error(
            "the operating system trust store held no usable certificates",
        ));
    }

    Ok(roots)
}
