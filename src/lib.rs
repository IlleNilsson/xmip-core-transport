//! Transport implementations that need nothing outside the standard library.
//!
//! Direction-neutral, per ADR-0010: one protocol, one implementation, and the
//! artifact decides whether it receives or sends. HTTP is the same protocol
//! whether Xmip is listening or calling, which is why the receive and send
//! sides are two methods on one trait rather than two repositories.
//!
//! Shaped to mirror `XmipTransportVtable` in `include/xmip_module.h` so that
//! moving an implementation across the C boundary later is mechanical rather
//! than a redesign.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::time::Duration;

/// Which directions an implementation supports.
///
/// Mirrors `XMIP_DIR_RECEIVE` and `XMIP_DIR_SEND`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Directions(u32);

impl Directions {
    pub const RECEIVE: Directions = Directions(1);
    pub const SEND: Directions = Directions(2);
    pub const BOTH: Directions = Directions(3);

    /// True when this implementation can receive.
    pub fn receives(self) -> bool {
        self.0 & 1 != 0
    }

    /// True when this implementation can send.
    pub fn sends(self) -> bool {
        self.0 & 2 != 0
    }
}

/// One Stream as it arrived, with where it came from.
///
/// `origin_uri` is historical fact and never changes, per ADR-0013. It says
/// where the bytes came from, not where they are now.
#[derive(Debug, Clone)]
pub struct Arrived {
    pub origin_uri: String,
    pub bytes: Vec<u8>,
}

/// A transport failure, carrying the one fact resilience needs.
///
/// `retryable` mirrors `XMIP_IS_RETRYABLE`: it is a property of the failure,
/// not of the call site, so `xmip-core-resilience` can decide without knowing
/// which implementation produced it.
#[derive(Debug)]
pub struct TransportError {
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({})",
            self.message,
            if self.retryable {
                "retryable"
            } else {
                "not retryable"
            }
        )
    }
}

impl std::error::Error for TransportError {}

/// Classify an I/O failure.
///
/// A blip is retryable; a missing file or a refused permission is not. Getting
/// this wrong is how a platform either gives up too early or retries forever.
fn classify(context: &str, error: &std::io::Error) -> TransportError {
    use std::io::ErrorKind::*;
    let retryable = matches!(
        error.kind(),
        Interrupted
            | WouldBlock
            | TimedOut
            | ConnectionReset
            | ConnectionAborted
            | ConnectionRefused
    );
    TransportError {
        message: format!("{context}: {error}"),
        retryable,
    }
}

pub type Result<T> = std::result::Result<T, TransportError>;

/// A peer that broke the protocol. Saying it again will not help, so never retryable.
fn protocol_error(message: impl Into<String>) -> TransportError {
    TransportError {
        message: message.into(),
        retryable: false,
    }
}

/// The largest single Stream Xmip will read off one connection.
const MAX_BODY: usize = 64 * 1024 * 1024;

/// The largest number of header lines Xmip will read before giving up.
const MAX_HEADERS: usize = 200;

/// Add the protocol's default port when the authority does not carry one.
///
/// The bracket check is what keeps `[::1]` from being read as host-and-port.
fn with_default_port(authority: &str, default: u16) -> String {
    let has_port = match authority.rfind(']') {
        Some(close) => authority[close + 1..].starts_with(':'),
        None => authority.contains(':'),
    };
    if has_port {
        authority.to_string()
    } else {
        format!("{authority}:{default}")
    }
}

/// Strip exactly one trailing line ending, CRLF or LF.
fn trim_eol(raw: &[u8]) -> &[u8] {
    let mut end = raw.len();
    if end > 0 && raw[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && raw[end - 1] == b'\r' {
        end -= 1;
    }
    &raw[..end]
}

/// Read the lines before the blank line that ends a header block.
fn read_head(reader: &mut impl BufRead) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    loop {
        let mut raw = Vec::new();
        let read = reader
            .read_until(b'\n', &mut raw)
            .map_err(|e| classify("reading a header line", &e))?;
        if read == 0 {
            break;
        }
        let line = String::from_utf8_lossy(trim_eol(&raw)).to_string();
        if line.is_empty() {
            break;
        }
        if lines.len() == MAX_HEADERS {
            return Err(protocol_error("more header lines than Xmip will read"));
        }
        lines.push(line);
    }
    Ok(lines)
}

/// Find one header value. HTTP field names are case-insensitive, so this is not
/// a convenience — a peer sending `content-length` is as correct as `Content-Length`.
fn header<'a>(lines: &'a [String], name: &str) -> Option<&'a str> {
    lines.iter().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

/// One protocol, both directions.
pub trait Transport {
    /// The standard token, as it appears in a repository name.
    fn name(&self) -> &'static str;

    /// Which directions this implementation actually supports.
    fn directions(&self) -> Directions;

    /// Take whatever has arrived. Returns an empty vector when nothing has.
    fn receive(&self) -> Result<Vec<Arrived>>;

    /// Deliver bytes to a target expressed in this protocol's own terms.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()>;
}

// ---------------------------------------------------------------------------
// file
// ---------------------------------------------------------------------------

/// Streams that arrive as files in a directory.
///
/// The polled case: nothing is pushed to Xmip, Xmip goes and looks. Identity is
/// therefore implied by the Receive Location rather than presented by a caller.
pub struct FileTransport {
    root: PathBuf,
}

impl FileTransport {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FileTransport { root: root.into() }
    }
}

impl Transport for FileTransport {
    fn name(&self) -> &'static str {
        "file"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            // A drop directory that does not exist yet is not a failure.
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(classify("reading the drop directory", &e)),
        };

        let mut arrived = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| classify("listing the drop directory", &e))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let bytes = fs::read(&path).map_err(|e| classify("reading a dropped file", &e))?;
            arrived.push(Arrived {
                origin_uri: file_uri(&path),
                bytes,
            });
        }
        Ok(arrived)
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let path = self.root.join(target);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| classify("creating the send directory", &e))?;
        }
        fs::write(&path, bytes).map_err(|e| classify("writing the sent file", &e))
    }
}

/// Render a path as a file URI.
///
/// A URI uses forward slashes on every platform, so a Windows path has to be
/// converted rather than printed. `char::from(92)` is a backslash, written this
/// way to keep the escape out of the literal.
fn file_uri(path: &std::path::Path) -> String {
    format!(
        "file:///{}",
        path.display().to_string().replace(char::from(92), "/")
    )
}

// ---------------------------------------------------------------------------
// tcp
// ---------------------------------------------------------------------------

/// Streams that arrive over a TCP connection, one Stream per connection.
///
/// The pushed case: a caller connects, so a transport-level identity exists.
pub struct TcpTransport {
    bind: String,
    accept_timeout: Option<Duration>,
}

impl TcpTransport {
    pub fn new(bind: impl Into<String>) -> Self {
        TcpTransport {
            bind: bind.into(),
            accept_timeout: None,
        }
    }

    /// Bind and report the address actually assigned.
    ///
    /// Binding to port 0 lets the operating system choose, which is what a test
    /// wants and what an operator never does.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        let listener =
            TcpListener::bind(&self.bind).map_err(|e| classify("binding the listener", &e))?;
        let local = listener
            .local_addr()
            .map_err(|e| classify("reading the bound address", &e))?;
        Ok((listener, local.to_string()))
    }

    /// Take one connection from an already-bound listener.
    pub fn accept_one(&self, listener: &TcpListener) -> Result<Arrived> {
        let (mut stream, peer) = listener
            .accept()
            .map_err(|e| classify("accepting a connection", &e))?;
        if let Some(timeout) = self.accept_timeout {
            stream
                .set_read_timeout(Some(timeout))
                .map_err(|e| classify("setting the read timeout", &e))?;
        }
        let mut bytes = Vec::new();
        stream
            .read_to_end(&mut bytes)
            .map_err(|e| classify("reading the connection", &e))?;
        Ok(Arrived {
            origin_uri: format!("tcp://{peer}"),
            bytes,
        })
    }
}

impl Transport for TcpTransport {
    fn name(&self) -> &'static str {
        "tcp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let (listener, _) = self.bind()?;
        Ok(vec![self.accept_one(&listener)?])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let mut stream =
            TcpStream::connect(target).map_err(|e| classify("connecting to the peer", &e))?;
        stream
            .write_all(bytes)
            .map_err(|e| classify("writing to the peer", &e))?;
        stream
            .flush()
            .map_err(|e| classify("flushing to the peer", &e))
    }
}

// ---------------------------------------------------------------------------
// udp
// ---------------------------------------------------------------------------

/// Streams that arrive as datagrams. One datagram is one Stream.
///
/// There is no reply channel and no delivery guarantee, which makes it the
/// clearest case of a transport that cannot answer: a Contract failure here is
/// audited and nothing more.
pub struct UdpTransport {
    bind: String,
    max_datagram: usize,
}

impl UdpTransport {
    pub fn new(bind: impl Into<String>) -> Self {
        UdpTransport {
            bind: bind.into(),
            max_datagram: 65_507,
        }
    }
}

impl Transport for UdpTransport {
    fn name(&self) -> &'static str {
        "udp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let socket = UdpSocket::bind(&self.bind).map_err(|e| classify("binding the socket", &e))?;
        let mut buffer = vec![0u8; self.max_datagram];
        let (read, peer) = socket
            .recv_from(&mut buffer)
            .map_err(|e| classify("receiving a datagram", &e))?;
        buffer.truncate(read);
        Ok(vec![Arrived {
            origin_uri: format!("udp://{peer}"),
            bytes: buffer,
        }])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let socket =
            UdpSocket::bind("0.0.0.0:0").map_err(|e| classify("binding the sending socket", &e))?;
        socket
            .send_to(bytes, target)
            .map_err(|e| classify("sending a datagram", &e))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// http
// ---------------------------------------------------------------------------

/// Streams that arrive as HTTP request bodies. One request is one Stream.
///
/// The pushed case *with a reply channel*, which is the distinction that matters
/// to custody: the caller is still holding the connection open, so a Contract
/// failure can be answered rather than only audited. UDP cannot do that, which
/// is why the two live in the same file and behave differently at the gate.
pub struct HttpTransport {
    bind: String,
}

impl HttpTransport {
    pub fn new(bind: impl Into<String>) -> Self {
        HttpTransport { bind: bind.into() }
    }

    /// Bind and report the address actually assigned.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        let listener =
            TcpListener::bind(&self.bind).map_err(|e| classify("binding the listener", &e))?;
        let local = listener
            .local_addr()
            .map_err(|e| classify("reading the bound address", &e))?;
        Ok((listener, local.to_string()))
    }

    /// Take one request from an already-bound listener and answer it.
    ///
    /// The answer is `202 Accepted`, deliberately. Xmip has taken the Stream into
    /// custody and has promised nothing else — which is exactly the state a Stream
    /// is in once the arrival gate has passed and before the Journey exists.
    pub fn accept_one(&self, listener: &TcpListener) -> Result<Arrived> {
        let (mut stream, peer) = listener
            .accept()
            .map_err(|e| classify("accepting a connection", &e))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| classify("cloning the connection", &e))?,
        );

        let head = read_head(&mut reader)?;
        let request_line = head
            .first()
            .ok_or_else(|| protocol_error("a connection that sent no request"))?;
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string();

        let length: usize = match header(&head, "content-length") {
            Some(value) => value.parse().map_err(|_| {
                protocol_error(format!("a content-length that is not a number: {value}"))
            })?,
            None => 0,
        };
        if length > MAX_BODY {
            return Err(protocol_error(format!(
                "a body of {} bytes, over the {} byte limit",
                length, MAX_BODY
            )));
        }

        let mut bytes = vec![0u8; length];
        reader
            .read_exact(&mut bytes)
            .map_err(|e| classify("reading the request body", &e))?;

        stream
            .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .map_err(|e| classify("answering the request", &e))?;
        stream
            .flush()
            .map_err(|e| classify("flushing the answer", &e))?;

        Ok(Arrived {
            origin_uri: format!("http://{peer}{path}"),
            bytes,
        })
    }
}

impl Transport for HttpTransport {
    fn name(&self) -> &'static str {
        "http"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let (listener, _) = self.bind()?;
        Ok(vec![self.accept_one(&listener)?])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let (secure, rest) = match target.strip_prefix("https://") {
            Some(rest) => (true, rest),
            None => match target.strip_prefix("http://") {
                Some(rest) => (false, rest),
                None => {
                    return Err(protocol_error(format!(
                        "an http target must begin with http:// or https:// — got {target}"
                    )))
                }
            },
        };
        let (authority, path) = match rest.find('/') {
            Some(cut) => (&rest[..cut], &rest[cut..]),
            None => (rest, "/"),
        };
        if authority.is_empty() {
            return Err(protocol_error(format!(
                "an http target with no host — got {target}"
            )));
        }
        let address = with_default_port(authority, if secure { 443 } else { 80 });

        let tcp =
            TcpStream::connect(&address).map_err(|e| classify("connecting to the server", &e))?;

        if secure {
            #[cfg(feature = "tls")]
            {
                let guarded = tls_client(host_of(authority), tcp)?;
                return exchange(guarded, authority, path, bytes);
            }
            #[cfg(not(feature = "tls"))]
            {
                drop(tcp);
                return Err(protocol_error(
                    "https was asked for and this build has no tls feature compiled in",
                ));
            }
        }

        exchange(tcp, authority, path, bytes)
    }
}

/// Write a request and read back the status, over anything that reads and writes.
///
/// Split out so plaintext and TLS take the same path. A protocol implemented
/// twice is a protocol that behaves two ways.
fn exchange<S: Read + Write>(
    mut stream: S,
    authority: &str,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {authority}\r\n\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        bytes.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| classify("writing the request head", &e))?;
    stream
        .write_all(bytes)
        .map_err(|e| classify("writing the request body", &e))?;
    stream
        .flush()
        .map_err(|e| classify("flushing the request", &e))?;

    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader
        .read_line(&mut status)
        .map_err(|e| classify("reading the status line", &e))?;
    let code: u16 = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| {
            protocol_error(format!("a status line Xmip cannot read: {}", status.trim()))
        })?;

    if (200..300).contains(&code) {
        Ok(())
    } else {
        // 5xx is the server's problem and may well pass on a second attempt.
        // 4xx is ours and will not, with two documented exceptions.
        Err(TransportError {
            message: format!("the server answered {code}"),
            retryable: code >= 500 || code == 408 || code == 429,
        })
    }
}

/// The host without its port, and without the brackets an IPv6 literal carries.
#[cfg(feature = "tls")]
fn host_of(authority: &str) -> &str {
    if let Some(close) = authority.rfind(']') {
        return &authority[1..close];
    }
    match authority.rfind(':') {
        Some(colon) => &authority[..colon],
        None => authority,
    }
}

/// Wrap a connection in TLS, verified against the operating system trust store.
///
/// The native store rather than a bundled root list, because the organisations
/// Xmip is aimed at run internal certificate authorities and expect their own
/// certificates to work without waiting for Xmip to ship a new root bundle.
#[cfg(feature = "tls")]
fn tls_client(
    host: &str,
    tcp: TcpStream,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
    use std::sync::Arc;

    let mut roots = rustls::RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for certificate in loaded.certs {
        // One unparsable certificate is not a reason to refuse every other
        // certificate in the store.
        let _ = roots.add(certificate);
    }
    if roots.is_empty() {
        return Err(protocol_error(
            "the operating system trust store held no usable certificates",
        ));
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| protocol_error(format!("selecting tls versions: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();

    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| protocol_error(format!("a server name tls cannot use: {host}")))?;
    let connection = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|e| protocol_error(format!("starting the tls session: {e}")))?;

    Ok(rustls::StreamOwned::new(connection, tcp))
}

// ---------------------------------------------------------------------------
// smtp
// ---------------------------------------------------------------------------

/// Write one command or reply, terminated as the protocol requires.
fn say(stream: &mut TcpStream, line: &str) -> Result<()> {
    stream
        .write_all(line.as_bytes())
        .map_err(|e| classify("writing a line", &e))?;
    stream
        .write_all(b"\r\n")
        .map_err(|e| classify("writing a line ending", &e))?;
    stream.flush().map_err(|e| classify("flushing a line", &e))
}

/// Read one reply, following continuation lines.
///
/// A multi-line reply is `250-first` … `250 last`. It is the space in the fourth
/// column that ends it, not the line ending — reading line-at-a-time and stopping
/// at the first newline is the classic way to hang an SMTP client forever.
fn read_reply(reader: &mut impl BufRead) -> Result<(u16, String)> {
    let mut text = String::new();
    loop {
        let mut raw = String::new();
        let read = reader
            .read_line(&mut raw)
            .map_err(|e| classify("reading a reply", &e))?;
        if read == 0 {
            return Err(protocol_error("a connection that closed mid-reply"));
        }
        let line = String::from_utf8_lossy(trim_eol(raw.as_bytes())).to_string();
        let code: u16 = line
            .get(..3)
            .and_then(|c| c.parse().ok())
            .ok_or_else(|| protocol_error(format!("a reply with no code: {line}")))?;
        let continued = line.as_bytes().get(3) == Some(&b'-');
        text.push_str(&line);
        if continued {
            text.push('\n');
            continue;
        }
        return Ok((code, text));
    }
}

/// Read one reply and insist on a particular code.
fn expect(reader: &mut impl BufRead, wanted: u16, step: &str) -> Result<()> {
    let (code, text) = read_reply(reader)?;
    if code == wanted {
        return Ok(());
    }
    Err(TransportError {
        message: format!("{step}: the server answered {text}"),
        // SMTP runs the opposite way round from HTTP: 4xx is the transient
        // failure and 5xx is the permanent one.
        retryable: (400..500).contains(&code),
    })
}

/// Read a DATA payload, undoing the dot-stuffing that protects a leading period.
fn read_data(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let mut raw = Vec::new();
        let read = reader
            .read_until(b'\n', &mut raw)
            .map_err(|e| classify("reading the message data", &e))?;
        if read == 0 {
            return Err(protocol_error("a connection that closed inside DATA"));
        }
        let line = trim_eol(&raw);
        if line == b"." {
            return Ok(bytes);
        }
        let line = if line.starts_with(b"..") {
            &line[1..]
        } else {
            line
        };
        if bytes.len() + line.len() > MAX_BODY {
            return Err(protocol_error("a message over the size Xmip will read"));
        }
        if !bytes.is_empty() {
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(line);
    }
}

/// Streams that arrive as mail. One message body is one Stream.
///
/// The envelope is addressing, not content: the recipient is Send Location
/// configuration and the relay is where Xmip hands the message over. That split
/// is why `send` takes a mailbox as its target and not a host.
pub struct SmtpTransport {
    bind: String,
    relay: String,
    from: String,
}

impl SmtpTransport {
    /// A receiving transport. There is nothing to relay through.
    pub fn receiving(bind: impl Into<String>) -> Self {
        SmtpTransport {
            bind: bind.into(),
            relay: String::new(),
            from: String::new(),
        }
    }

    /// A sending transport, relaying through one server as one sender.
    pub fn sending(relay: impl Into<String>, from: impl Into<String>) -> Self {
        SmtpTransport {
            bind: String::new(),
            relay: relay.into(),
            from: from.into(),
        }
    }

    /// Bind and report the address actually assigned.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        let listener =
            TcpListener::bind(&self.bind).map_err(|e| classify("binding the listener", &e))?;
        let local = listener
            .local_addr()
            .map_err(|e| classify("reading the bound address", &e))?;
        Ok((listener, local.to_string()))
    }

    /// Take one message from an already-bound listener.
    ///
    /// Enough of RFC 5321 to accept a message and no more. `MAIL FROM` is a
    /// presented identity and belongs at the identification gate, so this
    /// answers it and keeps nothing — `Arrived` carries where the bytes came
    /// from, and the envelope is not that.
    pub fn accept_one(&self, listener: &TcpListener) -> Result<Arrived> {
        let (mut stream, peer) = listener
            .accept()
            .map_err(|e| classify("accepting a connection", &e))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| classify("cloning the connection", &e))?,
        );

        say(&mut stream, "220 xmip ESMTP")?;

        let mut bytes = Vec::new();
        loop {
            let mut raw = String::new();
            let read = reader
                .read_line(&mut raw)
                .map_err(|e| classify("reading a command", &e))?;
            if read == 0 {
                break;
            }
            let command = String::from_utf8_lossy(trim_eol(raw.as_bytes())).to_string();
            let verb = command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_uppercase();

            match verb.as_str() {
                "" => continue,
                "HELO" => say(&mut stream, "250 xmip")?,
                "EHLO" => say(&mut stream, "250-xmip\r\n250 8BITMIME")?,
                "MAIL" | "RCPT" | "RSET" | "NOOP" => say(&mut stream, "250 ok")?,
                "DATA" => {
                    say(&mut stream, "354 end with a line containing only a period")?;
                    bytes = read_data(&mut reader)?;
                    say(&mut stream, "250 accepted")?;
                }
                "QUIT" => {
                    // A client that hangs up without waiting for the goodbye is
                    // rude, not broken. The Stream already arrived, so a failed
                    // farewell must not become a failed receive.
                    let _ = say(&mut stream, "221 closing");
                    break;
                }
                _ => say(&mut stream, "502 not implemented")?,
            }
        }

        Ok(Arrived {
            origin_uri: format!("smtp://{peer}"),
            bytes,
        })
    }
}

impl Transport for SmtpTransport {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn receive(&self) -> Result<Vec<Arrived>> {
        let (listener, _) = self.bind()?;
        Ok(vec![self.accept_one(&listener)?])
    }

    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let recipient = target.strip_prefix("mailto:").unwrap_or(target);
        if !recipient.contains('@') {
            return Err(protocol_error(format!(
                "an smtp target must be a mailbox — got {target}"
            )));
        }
        let address = with_default_port(&self.relay, 25);

        let mut stream =
            TcpStream::connect(&address).map_err(|e| classify("connecting to the relay", &e))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| classify("cloning the connection", &e))?,
        );

        expect(&mut reader, 220, "the greeting")?;
        say(&mut stream, "EHLO xmip")?;
        expect(&mut reader, 250, "EHLO")?;
        say(&mut stream, &format!("MAIL FROM:<{}>", self.from))?;
        expect(&mut reader, 250, "MAIL FROM")?;
        say(&mut stream, &format!("RCPT TO:<{recipient}>"))?;
        expect(&mut reader, 250, "RCPT TO")?;
        say(&mut stream, "DATA")?;
        expect(&mut reader, 354, "DATA")?;

        for line in bytes.split(|b| *b == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            // A line that starts with a period would otherwise end the message.
            if line.starts_with(b".") {
                stream
                    .write_all(b".")
                    .map_err(|e| classify("writing the message data", &e))?;
            }
            stream
                .write_all(line)
                .map_err(|e| classify("writing the message data", &e))?;
            stream
                .write_all(b"\r\n")
                .map_err(|e| classify("writing the message data", &e))?;
        }
        say(&mut stream, ".")?;
        expect(&mut reader, 250, "the end of data")?;
        say(&mut stream, "QUIT")?;
        // Wait for the goodbye before dropping the connection. Without this the
        // server is still writing 221 when the socket closes under it, and a
        // delivery that fully succeeded is reported as a broken pipe.
        let _ = read_reply(&mut reader);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("xmip-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("creating the scratch directory");
        dir
    }

    #[test]
    fn file_transport_declares_both_directions() {
        let t = FileTransport::new(std::env::temp_dir());
        assert!(t.directions().receives());
        assert!(t.directions().sends());
        assert_eq!(t.name(), "file");
    }

    #[test]
    fn file_receive_is_empty_when_the_directory_is_absent() {
        let t = FileTransport::new(std::env::temp_dir().join("xmip-definitely-not-here"));
        assert!(t
            .receive()
            .expect("absent directory is not a failure")
            .is_empty());
    }

    #[test]
    fn file_round_trip_carries_bytes_and_origin() {
        let dir = scratch("file-round-trip");
        let t = FileTransport::new(&dir);

        t.send("order-1001.edi", b"ISA*00*").expect("sending");
        let arrived = t.receive().expect("receiving");

        assert_eq!(arrived.len(), 1);
        assert_eq!(arrived[0].bytes, b"ISA*00*");
        assert!(arrived[0].origin_uri.starts_with("file:///"));
        assert!(arrived[0].origin_uri.contains("order-1001.edi"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_uri_uses_forward_slashes_everywhere() {
        let dir = scratch("file-uri");
        let t = FileTransport::new(&dir);

        t.send("order.edi", b"x").expect("sending");
        let arrived = t.receive().expect("receiving");

        assert!(!arrived[0].origin_uri.contains(char::from(92)));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tcp_round_trip_carries_bytes_and_peer() {
        let t = TcpTransport::new("127.0.0.1:0");
        let (listener, address) = t.bind().expect("binding");

        let sender = std::thread::spawn(move || {
            let s = TcpTransport::new("127.0.0.1:0");
            s.send(&address, b"hello over tcp").expect("sending");
        });

        let arrived = t.accept_one(&listener).expect("accepting");
        sender.join().expect("the sending thread panicked");

        assert_eq!(arrived.bytes, b"hello over tcp");
        assert!(arrived.origin_uri.starts_with("tcp://127.0.0.1:"));
    }

    #[test]
    fn http_round_trip_carries_the_body_and_the_path() {
        let receiver = HttpTransport::new("127.0.0.1:0");
        let (listener, address) = receiver.bind().expect("binding");

        let sender = std::thread::spawn(move || {
            let s = HttpTransport::new("127.0.0.1:0");
            s.send(&format!("http://{address}/orders"), b"<order/>")
                .expect("sending");
        });

        let arrived = receiver.accept_one(&listener).expect("accepting");
        sender.join().expect("the sending thread panicked");

        assert_eq!(arrived.bytes, b"<order/>");
        assert!(arrived.origin_uri.starts_with("http://127.0.0.1:"));
        assert!(arrived.origin_uri.ends_with("/orders"));
    }

    #[test]
    fn an_http_target_without_a_scheme_is_rejected() {
        let t = HttpTransport::new("127.0.0.1:0");
        let error = t.send("example.com/orders", b"").expect_err("no scheme");
        assert!(!error.retryable);
    }

    #[test]
    fn smtp_round_trip_survives_a_leading_period() {
        let receiver = SmtpTransport::receiving("127.0.0.1:0");
        let (listener, address) = receiver.bind().expect("binding");

        let sender = std::thread::spawn(move || {
            let s = SmtpTransport::sending(address, "xmip@example.com");
            // The third line starts with a period, which is the one byte
            // sequence that can end a message early if it is not stuffed.
            s.send("mailto:orders@example.com", b"Subject: one\r\n\r\n.hidden")
                .expect("sending");
        });

        let arrived = receiver.accept_one(&listener).expect("accepting");
        sender.join().expect("the sending thread panicked");

        assert_eq!(arrived.bytes, b"Subject: one\r\n\r\n.hidden");
        assert!(arrived.origin_uri.starts_with("smtp://127.0.0.1:"));
    }

    #[test]
    fn an_smtp_target_that_is_not_a_mailbox_is_rejected() {
        let t = SmtpTransport::sending("127.0.0.1:25", "xmip@example.com");
        let error = t.send("127.0.0.1:25", b"").expect_err("not a mailbox");
        assert!(!error.retryable);
    }

    #[test]
    fn a_default_port_is_added_only_when_one_is_missing() {
        assert_eq!(with_default_port("example.com", 80), "example.com:80");
        assert_eq!(
            with_default_port("example.com:8080", 80),
            "example.com:8080"
        );
        assert_eq!(with_default_port("[::1]", 80), "[::1]:80");
        assert_eq!(with_default_port("[::1]:8080", 80), "[::1]:8080");
    }

    #[test]
    fn a_missing_file_is_not_retryable() {
        let error = classify(
            "reading",
            &std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        );
        assert!(!error.retryable);
    }

    #[test]
    fn a_refused_connection_is_retryable() {
        let error = classify(
            "connecting",
            &std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
        );
        assert!(error.retryable);
    }
}
