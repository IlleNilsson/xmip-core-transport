//! Opening sockets the way every TCP and UDP technology opens them: bind
//! and report the address actually assigned, accept or connect with the
//! read timeout applied, split a connection into a buffered reader and a
//! writer, and read a `scheme://authority/path` target.
//!
//! Twenty technologies wrote these same fifteen lines each before this
//! file existed (2026-09-08). What a protocol does *with* the socket stays
//! in the technology; what it takes to have one is here.

use std::io::{BufReader, ErrorKind};
use std::net::{
    Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, ToSocketAddrs, UdpSocket,
};
use std::time::{Duration, Instant};

use crate::error::{Result, TransportError, classify};

/// Bind a listener at `bind` and report the address actually assigned —
/// what `127.0.0.1:0` became.
///
/// # Errors
/// Where the address is taken, malformed, or not permitted.
pub fn bind_tcp(bind: &str) -> Result<(TcpListener, String)> {
    let listener = TcpListener::bind(bind).map_err(|e| classify("binding the listener", &e))?;
    let local = listener
        .local_addr()
        .map_err(|e| classify("reading the bound address", &e))?;
    Ok((listener, local.to_string()))
}

/// Accept one connection within `timeout`, with `timeout` on its reads.
/// `None` waits forever, which is what a listening Receive Location does.
///
/// The wait for the connection is bounded as well as the reads. It was not
/// until 2026-09-20, and the cost was the Playground's own test suite: a far
/// end stood up for one exchange blocked in `accept` for good when its near
/// end never connected, so a run that should have failed in two seconds hung
/// with the process at zero percent and thirty-six loopback connections open.
/// `TcpTransport::loopback` had called this the *loopback timeout on the
/// accept* since it was written, and a read timeout is not that. A test that
/// hangs never fails, which is the worst way for a gate to be wrong.
///
/// # Errors
/// Where nothing connected within `timeout`, where the connection could not
/// be accepted, or where the timeout could not be set.
pub fn accept_tcp(
    listener: &TcpListener,
    timeout: Option<Duration>,
) -> Result<(TcpStream, SocketAddr)> {
    let (stream, peer) = accept_within(listener, timeout)?;
    settle(&stream, timeout)?;
    Ok((stream, peer))
}

// Polled rather than selected on: std has no accept with a deadline, and a
// two-millisecond nap costs a thousandth of the shortest timeout anyone
// passes while keeping this dependency-free. The listener is put back into
// blocking mode on every way out, including the error ways, because it
// belongs to the caller and a non-blocking listener it did not ask for would
// fail somewhere it could not explain.
fn accept_within(
    listener: &TcpListener,
    timeout: Option<Duration>,
) -> Result<(TcpStream, SocketAddr)> {
    let Some(timeout) = timeout else {
        return listener
            .accept()
            .map_err(|e| classify("accepting a connection", &e));
    };

    listener
        .set_nonblocking(true)
        .map_err(|e| classify("waiting for a connection", &e))?;

    let deadline = Instant::now() + timeout;
    let accepted = loop {
        match listener.accept() {
            Ok(pair) => break Ok(pair),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    break Err(TransportError::retryable(format!(
                        "nothing connected within {} ms",
                        timeout.as_millis()
                    ))
                    .at("accepting a connection"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => break Err(classify("accepting a connection", &error)),
        }
    };

    listener
        .set_nonblocking(false)
        .map_err(|e| classify("waiting for a connection", &e))?;

    let (stream, peer) = accepted?;

    stream
        .set_nonblocking(false)
        .map_err(|e| classify("settling the accepted connection", &e))?;

    Ok((stream, peer))
}

/// Connect to `target` within `timeout`, with `timeout` on the reads.
/// `None` waits as long as the operating system does.
///
/// The connect is bounded as well as the reads, for the reason
/// [`accept_tcp`] is: an unbounded wait turns a failure into a hang, and a
/// hang has no verdict. `TcpStream::connect` waits on the operating system's
/// own schedule — on Windows the SYN retry alone is seconds, and a machine
/// out of ephemeral ports waits longer still with nothing to show for it.
///
/// # Errors
/// Where the peer refused, could not be reached within `timeout`, or the
/// target does not resolve — retryable, as a connection refused is.
pub fn connect_tcp(target: &str, timeout: Option<Duration>) -> Result<TcpStream> {
    let stream = match timeout {
        None => TcpStream::connect(target).map_err(|e| classify("connecting to the peer", &e))?,
        Some(within) => {
            let address = target
                .to_socket_addrs()
                .map_err(|e| classify("resolving the peer", &e))?
                .next()
                .ok_or_else(|| {
                    TransportError::permanent(format!("{target} names no address"))
                        .at("connecting to the peer")
                })?;

            TcpStream::connect_timeout(&address, within)
                .map_err(|e| classify("connecting to the peer", &e))?
        }
    };

    settle(&stream, timeout)?;
    Ok(stream)
}

/// Apply `timeout` to a connection's reads; `None` waits forever.
///
/// # Errors
/// Where the socket refused the option.
pub fn settle(stream: &TcpStream, timeout: Option<Duration>) -> Result<()> {
    if let Some(timeout) = timeout {
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| classify("setting the read timeout", &e))?;
    }
    Ok(())
}

/// A connection as a buffered reader and a writer, so a protocol can read
/// lines from one hand and write from the other.
///
/// # Errors
/// Where the socket could not be cloned.
pub fn split(stream: TcpStream) -> Result<(BufReader<TcpStream>, TcpStream)> {
    let writer = stream
        .try_clone()
        .map_err(|e| classify("cloning the connection", &e))?;
    Ok((BufReader::new(stream), writer))
}

/// Bind a datagram socket at `bind` with `timeout` on its receives, and
/// report the address actually assigned.
///
/// # Errors
/// Where the address is taken, malformed, or not permitted.
pub fn bind_udp(bind: &str, timeout: Option<Duration>) -> Result<(UdpSocket, String)> {
    let socket = UdpSocket::bind(bind).map_err(|e| classify("binding the socket", &e))?;
    if let Some(timeout) = timeout {
        socket
            .set_read_timeout(Some(timeout))
            .map_err(|e| classify("setting the receive timeout", &e))?;
    }
    let local = socket
        .local_addr()
        .map_err(|e| classify("reading the bound address", &e))?;
    Ok((socket, local.to_string()))
}

/// The group and the `0.0.0.0:port` to bind for it, where `bind` is a
/// multicast address; `None` where it is a unicast one.
#[must_use]
pub fn multicast_group(bind: &str) -> Option<(Ipv4Addr, String)> {
    let address: SocketAddrV4 = bind.parse().ok()?;
    address
        .ip()
        .is_multicast()
        .then(|| (*address.ip(), format!("0.0.0.0:{}", address.port())))
}

/// Bind a datagram socket at `bind` with `timeout` on its receives, and
/// where `bind` is a multicast address bind its port on every interface
/// and join the group. mDNS and SSDP both live on a group and both wrote
/// this before 2026-09-14 (ADR-0044); a test binds `127.0.0.1:0` and
/// never joins.
///
/// # Errors
/// Where the address is taken, malformed, or not permitted, or the group
/// could not be joined.
pub fn bind_multicast(bind: &str, timeout: Option<Duration>) -> Result<(UdpSocket, String)> {
    let Some((group, port)) = multicast_group(bind) else {
        return bind_udp(bind, timeout);
    };
    let (socket, local) = bind_udp(&port, timeout)?;
    socket
        .join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED)
        .map_err(|e| classify("joining the group", &e))?;
    Ok((socket, local))
}

/// `scheme://authority/path` split into its authority and path, where
/// `target` opens with `scheme://`; `None` where it does not, so the caller
/// falls back to what it was configured with.
#[must_use]
pub fn target<'a>(scheme: &str, target: &'a str) -> Option<(&'a str, &'a str)> {
    let rest = target.strip_prefix(scheme)?.strip_prefix("://")?;
    Some(rest.split_once('/').unwrap_or((rest, "")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Write};

    #[test]
    fn an_accept_nobody_answers_gives_up_within_its_timeout() {
        // The defect this asserts hung the Playground's whole suite: a far
        // end stood up for one exchange waited in `accept` for good when its
        // near end never connected (2026-09-20). It must fail, and it must
        // fail inside the time it was given rather than a moment before the
        // heat death of the universe.
        let (listener, _) = bind_tcp("127.0.0.1:0").expect("bind");
        let began = Instant::now();
        let refused = accept_tcp(&listener, Some(Duration::from_millis(200)));
        let waited = began.elapsed();

        assert!(refused.is_err(), "nothing connected, so nothing arrived");
        assert!(
            waited < Duration::from_secs(2),
            "gave up after {waited:?}, which is not a timeout"
        );
        assert!(
            waited >= Duration::from_millis(150),
            "gave up after {waited:?}, before it had waited"
        );
    }

    #[test]
    fn a_listener_is_blocking_again_after_a_bounded_accept() {
        // The listener is the caller's. A non-blocking one handed back would
        // fail in whatever the caller did next, far from here.
        let (listener, address) = bind_tcp("127.0.0.1:0").expect("bind");
        assert!(accept_tcp(&listener, Some(Duration::from_millis(100))).is_err());

        let client = std::thread::spawn(move || {
            let mut stream = connect_tcp(&address, Some(Duration::from_secs(2))).expect("connect");
            stream.write_all(b"after").expect("write");
        });
        let (stream, _) = accept_tcp(&listener, Some(Duration::from_secs(2))).expect("accept");

        assert!(!stream.peer_addr().expect("peer").ip().is_unspecified());
        client.join().expect("the client");
    }

    #[test]
    fn a_bound_listener_accepts_a_settled_connection_that_splits() {
        let (listener, address) = bind_tcp("127.0.0.1:0").expect("bind");
        assert!(address.starts_with("127.0.0.1:"));
        assert!(!address.ends_with(":0"), "the port actually assigned");
        let client = std::thread::spawn(move || {
            let stream = connect_tcp(&address, Some(Duration::from_secs(2))).expect("connect");
            let (mut reader, mut writer) = split(stream).expect("split");
            writer.write_all(b"hello\n").expect("write");
            let mut line = String::new();
            reader.read_line(&mut line).expect("read");
            line
        });
        let (stream, peer) = accept_tcp(&listener, Some(Duration::from_secs(2))).expect("accept");
        assert!(peer.ip().is_loopback());
        assert_eq!(
            stream.read_timeout().expect("timeout"),
            Some(Duration::from_secs(2))
        );
        let (mut reader, mut writer) = split(stream).expect("split");
        let mut line = String::new();
        reader.read_line(&mut line).expect("read");
        assert_eq!(line, "hello\n");
        writer.write_all(b"back\n").expect("write");
        assert_eq!(client.join().expect("thread"), "back\n");
    }

    #[test]
    fn a_refused_connection_is_retryable_and_a_datagram_socket_binds() {
        let (listener, address) = bind_tcp("127.0.0.1:0").expect("bind");
        drop(listener);
        let error = connect_tcp(&address, None).expect_err("refused");
        assert!(error.retryable);
        let (socket, address) =
            bind_udp("127.0.0.1:0", Some(Duration::from_millis(50))).expect("udp");
        assert!(!address.ends_with(":0"));
        let mut buffer = [0u8; 8];
        assert!(socket.recv(&mut buffer).is_err(), "times out");
    }

    #[test]
    fn a_multicast_group_is_known_and_a_unicast_bind_joins_nothing() {
        let (group, port) = multicast_group("224.0.0.251:5353").expect("multicast");
        assert_eq!(group, Ipv4Addr::new(224, 0, 0, 251));
        assert_eq!(port, "0.0.0.0:5353");
        assert!(multicast_group("127.0.0.1:5353").is_none());
        assert!(multicast_group("nonsense").is_none());
        let (_, address) = bind_multicast("127.0.0.1:0", None).expect("unicast");
        assert!(address.starts_with("127.0.0.1:") && !address.ends_with(":0"));
    }

    #[test]
    fn a_target_splits_on_its_scheme() {
        assert_eq!(
            target("mqtt", "mqtt://host:1883/a/b"),
            Some(("host:1883", "a/b"))
        );
        assert_eq!(target("mqtt", "mqtt://host"), Some(("host", "")));
        assert_eq!(target("mqtt", "nats://host/x"), None);
        assert_eq!(target("mqtt", "a/b"), None);
    }
}
