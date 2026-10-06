//! Opening sockets the way every TCP and UDP technology opens them: bind
//! and report the address actually assigned, accept or connect with the
//! read timeout applied, split a connection into a buffered reader and a
//! writer. Datagrams are sent from a socket bound once ([`crate::sender`]).
//! A target a send names is read by `net::Target`; this file split one on
//! its first slash, the query left in the path, until 2026-09-28.
//!
//! Twenty technologies wrote these same fifteen lines each before this
//! file existed (2026-09-08). What a protocol does *with* the socket stays
//! in the technology; what it takes to have one is here.

use std::io::{self, BufReader, ErrorKind};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
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

/// Accept one connection within `timeout`, with `timeout` on its reads and
/// Nagle's algorithm off. `None` waits forever, which is what a listening Receive Location does.
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
    let (stream, peer) = accept_within(listener, timeout, "nothing connected")?;
    settle(&stream, timeout)?;
    // An answer written as a head and then a body goes at once, as
    // `net::connect` has a request go, not after the delayed acknowledgement.
    stream
        .set_nodelay(true)
        .map_err(|e| classify("setting the connection's delay", &e))?;
    Ok((stream, peer))
}

/// A listener whose accept [`accept_within`] can bound: a TCP listener, a
/// Unix domain socket's, a Windows named pipe's.
pub trait Acceptor {
    /// What one accept hands back.
    type Accepted;

    /// One accept, blocking or not as the listener is set.
    ///
    /// # Errors
    /// `WouldBlock` where the listener is non-blocking and nobody is there.
    fn accept_now(&self) -> io::Result<Self::Accepted>;

    /// Put the listener into non-blocking mode, or back out of it.
    ///
    /// # Errors
    /// Where the operating system refused.
    fn nonblocking(&self, nonblocking: bool) -> io::Result<()>;

    /// Hand an accepted connection back blocking, whatever it inherited.
    ///
    /// # Errors
    /// Where the operating system refused.
    fn settle(accepted: &Self::Accepted) -> io::Result<()>;

    /// Wait until a peer is there to accept, or `within` has passed.
    ///
    /// # Errors
    /// Where the wait itself failed.
    fn wait_ready(&self, within: Duration) -> io::Result<()>;
}

impl Acceptor for TcpListener {
    type Accepted = (TcpStream, SocketAddr);

    fn accept_now(&self) -> io::Result<Self::Accepted> {
        // bounded: the caller's, accept_within's deadline or its None arm
        self.accept()
    }

    fn nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.set_nonblocking(nonblocking)
    }

    fn settle((stream, _): &Self::Accepted) -> io::Result<()> {
        stream.set_nonblocking(false)
    }

    fn wait_ready(&self, within: Duration) -> io::Result<()> {
        readable(self, within)
    }
}

#[cfg(unix)]
impl Acceptor for std::os::unix::net::UnixListener {
    type Accepted = (
        std::os::unix::net::UnixStream,
        std::os::unix::net::SocketAddr,
    );

    fn accept_now(&self) -> io::Result<Self::Accepted> {
        // bounded: the caller's, accept_within's deadline or its None arm
        self.accept()
    }

    fn nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.set_nonblocking(nonblocking)
    }

    fn settle((stream, _): &Self::Accepted) -> io::Result<()> {
        stream.set_nonblocking(false)
    }

    fn wait_ready(&self, within: Duration) -> io::Result<()> {
        readable(self, within)
    }
}

/// Block until `source` has something to read — for a listener, a peer to
/// accept — or `within` passes: the operating system's own wait, poll(2)
/// or `WSAPoll`, so an arrival is taken the moment it lands.
fn readable(source: &impl rustix::fd::AsFd, within: Duration) -> io::Result<()> {
    ready(&[source.as_fd()], Some(within)).map(drop)
}

/// Block until one of `sources` has something to read, or has hung up, or
/// `within` passes (`None` waits as long as it takes): which of them did,
/// in the order given. One wait over a listener and the connections kept
/// on it is how a Receive Location takes whichever peer spoke first
/// ([`crate::serving`]).
///
/// # Errors
/// Where the wait itself failed.
pub(crate) fn ready(
    sources: &[rustix::fd::BorrowedFd<'_>],
    within: Option<Duration>,
) -> io::Result<Vec<bool>> {
    // A wait too long to write is a year: the deadline ends it long before.
    let year = Duration::from_hours(365 * 24);
    let timeout = within
        .map(|within| rustix::event::Timespec::try_from(within.min(year)))
        .transpose()
        .map_err(|_| io::Error::from(ErrorKind::InvalidInput))?;
    let mut polled: Vec<_> = sources
        .iter()
        .map(|source| rustix::event::PollFd::new(source, rustix::event::PollFlags::IN))
        .collect();
    rustix::event::poll(&mut polled, timeout.as_ref())?;
    Ok(polled.iter().map(|one| !one.revents().is_empty()).collect())
}

/// Accept one connection on `listener` within `timeout`; `None` waits as
/// long as it takes, which is what a listening Receive Location does.
/// `nobody` says what did not happen, in the failure: *nothing connected*,
/// *no writer came*.
///
/// The one bounded accept in the estate. Until 2026-09-27 the capability,
/// the named pipe and the Unix domain socket each polled a non-blocking
/// accept with a two-millisecond nap between tries, so a connection that
/// arrived waited up to two milliseconds for nothing. This waits on the
/// listener's readiness instead and takes the connection as it lands. The
/// listener is non-blocking while it waits, so a peer that vanishes between
/// readiness and accept is a `WouldBlock` and another wait, never a hang;
/// it is handed back blocking on every way out, error ways included,
/// because it belongs to the caller.
///
/// # Errors
/// Where nothing arrived within `timeout` (retryable), the accept failed,
/// or the listener's mode could not be set.
pub fn accept_within<A: Acceptor>(
    listener: &A,
    timeout: Option<Duration>,
    nobody: &str,
) -> Result<A::Accepted> {
    let Some(timeout) = timeout else {
        return listener
            // bounded: the None arm: a listening Receive Location waits as long as it runs
            .accept_now()
            .map_err(|e| classify("accepting a connection", &e));
    };
    listener
        .nonblocking(true)
        .map_err(|e| classify("waiting for a connection", &e))?;
    let deadline = Instant::now() + timeout;
    let accepted = loop {
        // bounded: non-blocking, each wait inside the deadline above
        match listener.accept_now() {
            Ok(accepted) => break Ok(accepted),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break Err(TransportError::retryable(format!(
                        "{nobody} within {} ms",
                        timeout.as_millis()
                    ))
                    .at("accepting a connection"));
                }
                match listener.wait_ready(left) {
                    Err(error) if error.kind() != ErrorKind::Interrupted => {
                        break Err(classify("waiting for a connection", &error));
                    }
                    _ => {}
                }
            }
            Err(error) => break Err(classify("accepting a connection", &error)),
        }
    };
    listener
        .nonblocking(false)
        .map_err(|e| classify("waiting for a connection", &e))?;
    let accepted = accepted?;
    A::settle(&accepted).map_err(|e| classify("settling the accepted connection", &e))?;
    Ok(accepted)
}

/// Connect to `target` within `timeout`, with `timeout` on the reads and
/// writes: every address `target` resolves to, in turn, inside the one
/// deadline (`net::connect`, the estate's one TCP connect). `None` waits
/// as long as the operating system does.
///
/// The connect is bounded as well as the reads, for the reason
/// [`accept_tcp`] is: an unbounded wait turns a failure into a hang, and a
/// hang has no verdict. `TcpStream::connect` waits on the operating system's
/// own schedule — on Windows the SYN retry alone is seconds, and a machine
/// out of ephemeral ports waits longer still with nothing to show for it.
///
/// # Errors
/// Where the peer refused or could not be reached within `timeout` —
/// retryable, as a connection refused is — or the target does not resolve.
pub fn connect_tcp(target: &str, timeout: Option<Duration>) -> Result<TcpStream> {
    Ok(net::connect(target, timeout)?)
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
    fn a_connection_is_accepted_as_it_lands_not_after_a_nap() {
        // Each round the accept is waiting before the peer connects, so a
        // polled accept napped once a round; a readiness wait does not.
        // Until 2026-09-27 the nap was two milliseconds, and a hundred
        // rounds took two hundred.
        const ROUNDS: u32 = 100;
        let (listener, address) = bind_tcp("127.0.0.1:0").expect("bind");
        let (go, went) = std::sync::mpsc::channel::<()>();
        let (landed, lands) = std::sync::mpsc::channel::<Instant>();
        let peer = std::thread::spawn(move || {
            for _ in 0..ROUNDS {
                went.recv().expect("go");
                let mut stream = connect_tcp(&address, Some(Duration::from_secs(5))).expect("c");
                landed.send(Instant::now()).expect("landed");
                let mut back = [0u8; 1];
                std::io::Read::read_exact(&mut stream, &mut back).expect("answered");
            }
        });
        // What the accept adds once the connection has landed: the peer's
        // wake-up and its handshake are the peer's, not the accept's, and
        // under the other tests running beside this one they are load.
        let mut waited = Duration::ZERO;
        for _ in 0..ROUNDS {
            go.send(()).expect("go");
            let (mut stream, _) =
                accept_tcp(&listener, Some(Duration::from_secs(5))).expect("accepted");
            let accepted = Instant::now();
            let connected = lands.recv().expect("landed");
            waited += accepted.saturating_duration_since(connected);
            stream.write_all(b"x").expect("answer");
        }
        peer.join().expect("peer");
        assert!(
            waited < Duration::from_millis(u64::from(ROUNDS)),
            "{ROUNDS} accepts waited {waited:?}, over a millisecond each"
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
        assert!(stream.nodelay().expect("nodelay"), "Nagle's algorithm off");
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
}
