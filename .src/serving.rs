//! A Receive Location's kept listener and the connections its peers keep
//! open on it: every receive takes the next exchange from whichever peer
//! sends first, a new one or one already connected.
//!
//! A sender that keeps its connection between messages — every HTTP sender
//! since 2026-09-27 — sends its next message on the connection it already
//! has. A receiver that answered one request and closed made each of those
//! a new connection, and one that served a connection until its peer closed
//! it held every message on it until then. Here the connection is kept
//! after its exchange, beside the listener ([`crate::kept`]), and one wait
//! on the readiness of all of them ([`crate::socket`]) hands the receive
//! whichever spoke: the listener's new peer is accepted and kept, a kept
//! connection's next exchange is taken. What one exchange on a connection
//! is — an HTTP request answered, in whichever version the connection
//! speaks — is the technology's, handed in; keeping and waiting are here.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use rustix::fd::AsFd;

use crate::error::{Result, TransportError, classify};
use crate::kept::Kept;
use crate::socket;

/// The most connections kept open at once. One more accepted lets the one
/// kept longest go, so a crowd of idle peers costs a bounded number of
/// sockets; a peer let go connects again.
const MOST_KEPT: usize = 64;

/// A connection a peer keeps open between exchanges.
pub trait Open: Send {
    /// The socket whose readiness says the peer sent something, or hung up.
    fn socket(&self) -> &TcpStream;

    /// Whether an exchange already read waits in this side's buffers, where
    /// the socket's readiness cannot see it: a pipelined request, a second
    /// stream finished in the same read.
    fn waiting(&self) -> bool {
        false
    }
}

/// What one turn on a connection came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn<T> {
    /// An exchange was taken, and the connection is kept for the next.
    Taken(T),
    /// An exchange was taken, and it was the connection's last: the peer
    /// said so, or the protocol does.
    Last(T),
    /// What was read is not an exchange yet — a setting, a ping, the first
    /// half of a frame — and the connection waits for more.
    Nothing,
    /// The peer closed the connection.
    Closed,
}

/// A kept listener and the connections kept open on it.
pub struct Serving<C> {
    listener: Kept<TcpListener>,
    open: Mutex<Vec<C>>,
}

impl<C: Open> Serving<C> {
    /// Nothing bound, nothing open.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            listener: Kept::new(),
            open: Mutex::new(Vec::new()),
        }
    }

    /// Where the listener is bound: `None` before the first receive.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        self.listener.address()
    }

    /// Bind the listener by `bind` now, where the first receive has not,
    /// and say where it is: for a Location bound at configuration, and a
    /// test that sends before it receives.
    ///
    /// # Errors
    /// As `bind`.
    pub fn bound(&self, bind: impl FnOnce() -> Result<(TcpListener, String)>) -> Result<&str> {
        self.listener.bound(bind)?;
        Ok(self.listener.address().unwrap_or_default())
    }

    /// How many connections are kept open now.
    #[must_use]
    pub fn open(&self) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// The next exchange, from whichever peer sends first within `timeout`
    /// (`None` waits as long as it takes, which is what a listening Receive
    /// Location does): the listener bound by `bind` where it is not yet, a
    /// new peer made a connection by `opened` and kept, and a turn taken on
    /// a connection by `exchange`. A connection's timeout on its reads is
    /// `timeout` too, so a peer that stops mid-exchange is let go.
    ///
    /// # Errors
    /// Where the listener could not be bound, nothing arrived within
    /// `timeout` (retryable), or a turn failed — whose connection is let go.
    pub fn next<T>(
        &self,
        bind: impl FnOnce() -> Result<(TcpListener, String)>,
        timeout: Option<Duration>,
        mut opened: impl FnMut(TcpStream, SocketAddr) -> Result<C>,
        mut exchange: impl FnMut(&mut C) -> Result<Turn<T>>,
    ) -> Result<T> {
        let listener = self.listener.bound(bind)?;
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            // What is already read goes first: no socket will say so.
            let Some(at) = open.iter().position(Open::waiting).map_or_else(
                || spoken(listener, &mut open, deadline, timeout, &mut opened),
                |at| Ok(Some(at)),
            )?
            else {
                continue;
            };
            match exchange(&mut open[at]) {
                Ok(Turn::Taken(taken)) => {
                    // The one served goes to the back: the next wait
                    // favours the others, and the one let go when too
                    // many are kept is the one quiet longest.
                    let served = open.remove(at);
                    open.push(served);
                    return Ok(taken);
                }
                Ok(Turn::Last(taken)) => {
                    open.remove(at);
                    return Ok(taken);
                }
                Ok(Turn::Nothing) => {}
                Ok(Turn::Closed) => {
                    open.remove(at);
                }
                Err(error) => {
                    open.remove(at);
                    return Err(error);
                }
            }
        }
    }
}

/// Wait until the deadline for the listener or an open connection: the
/// first connection that spoke, if one did. A new peer is accepted and kept
/// whenever the listener is ready, so busy connections never keep one
/// waiting; the one quiet longest is let go where too many are kept.
fn spoken<C: Open>(
    listener: &TcpListener,
    open: &mut Vec<C>,
    deadline: Option<Instant>,
    timeout: Option<Duration>,
    opened: &mut impl FnMut(TcpStream, SocketAddr) -> Result<C>,
) -> Result<Option<usize>> {
    let left = deadline.map(|at| at.saturating_duration_since(Instant::now()));
    if left.is_some_and(|left| left.is_zero()) {
        return Err(nothing_within(timeout));
    }
    let ready = wait(listener, open, left)?;
    let mut spoke = ready[1..].iter().position(|spoke| *spoke);
    if ready[0] {
        let (stream, peer) = socket::accept_tcp(listener, timeout)?;
        if open.len() == MOST_KEPT {
            open.remove(0);
            spoke = spoke.and_then(|at| at.checked_sub(1));
        }
        open.push(opened(stream, peer)?);
    }
    Ok(spoke)
}

/// Which of the listener and the open connections is ready, the listener
/// first.
fn wait<C: Open>(
    listener: &TcpListener,
    open: &[C],
    within: Option<Duration>,
) -> Result<Vec<bool>> {
    let mut sources = Vec::with_capacity(open.len() + 1);
    sources.push(listener.as_fd());
    sources.extend(open.iter().map(|connection| connection.socket().as_fd()));
    loop {
        match socket::ready(&sources, within) {
            Ok(ready) => return Ok(ready),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(classify("waiting for a peer", &error)),
        }
    }
}

fn nothing_within(timeout: Option<Duration>) -> TransportError {
    let within = timeout.unwrap_or_default().as_millis();
    TransportError::retryable(format!("nothing arrived within {within} ms"))
        .at("waiting for a peer")
}

impl<C: Open> Default for Serving<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// A copy of a transport binds and keeps its own, as [`Kept`] does.
impl<C: Open> Clone for Serving<C> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl<C> std::fmt::Debug for Serving<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Serving")
            .field("address", &self.listener.address())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const WAIT: Option<Duration> = Some(Duration::from_secs(2));

    /// A protocol whose exchange is a line, answered with `ok`; `bye` is
    /// the connection's last.
    struct Lines {
        reader: BufReader<TcpStream>,
        peer: SocketAddr,
    }

    impl Open for Lines {
        fn socket(&self) -> &TcpStream {
            self.reader.get_ref()
        }

        fn waiting(&self) -> bool {
            !self.reader.buffer().is_empty()
        }
    }

    fn line(lines: &mut Lines) -> Result<Turn<(String, SocketAddr)>> {
        let mut line = String::new();
        let read = lines
            .reader
            .read_line(&mut line)
            .map_err(|e| classify("reading", &e))?;
        if read == 0 {
            return Ok(Turn::Closed);
        }
        lines
            .reader
            .get_mut()
            .write_all(b"ok\n")
            .map_err(|e| classify("answering", &e))?;
        let line = line.trim_end().to_string();
        Ok(if line == "bye" {
            Turn::Last((line, lines.peer))
        } else {
            Turn::Taken((line, lines.peer))
        })
    }

    /// The connection a new peer is, its peer read off the socket as a
    /// technology whose origin names it would.
    fn opened(stream: TcpStream, accepted: SocketAddr) -> Result<Lines> {
        let peer = stream
            .peer_addr()
            .map_err(|e| classify("reading the peer", &e))?;
        assert_eq!(peer, accepted);
        Ok(Lines {
            reader: BufReader::new(stream),
            peer,
        })
    }

    struct Peer {
        reader: BufReader<TcpStream>,
    }

    impl Peer {
        fn to(address: &str) -> Self {
            let stream = socket::connect_tcp(address, WAIT).expect("connect");
            Self {
                reader: BufReader::new(stream),
            }
        }

        fn say(&mut self, line: &str) {
            let written = format!("{line}\n");
            self.reader
                .get_mut()
                .write_all(written.as_bytes())
                .expect("say");
        }

        fn heard(&mut self) -> String {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("hear");
            line
        }
    }

    fn serving() -> (Serving<Lines>, String) {
        let serving = Serving::new();
        let binds = AtomicUsize::new(0);
        let bind = || {
            binds.fetch_add(1, Ordering::SeqCst);
            socket::bind_tcp("127.0.0.1:0")
        };
        // Bound before any peer, as the first receive binds it.
        drop(serving.next(bind, Some(Duration::from_millis(1)), opened, line));
        let address = serving.address().expect("bound").to_string();
        let again = serving.bound(|| panic!("bound twice")).expect("kept");
        assert_eq!(again, address);
        assert_eq!(binds.load(Ordering::SeqCst), 1);
        (serving, address)
    }

    fn next(serving: &Serving<Lines>) -> Result<(String, SocketAddr)> {
        let unbound = || -> Result<(TcpListener, String)> { panic!("bound twice") };
        serving.next(unbound, WAIT, opened, line)
    }

    #[test]
    fn one_connection_carries_many_exchanges_one_per_receive() {
        let (serving, address) = serving();
        let mut peer = Peer::to(&address);
        // Said before any receive: the connection is queued and the lines
        // wait in its socket.
        peer.say("one");
        peer.say("two");
        let (first, from) = next(&serving).expect("first");
        let (second, again) = next(&serving).expect("second");
        assert_eq!((first.as_str(), second.as_str()), ("one", "two"));
        assert_eq!(from, again, "the same connection");
        assert_eq!(serving.open(), 1);
        assert_eq!(peer.heard(), "ok\n");
        peer.say("three");
        assert_eq!(next(&serving).expect("third").0, "three");
    }

    #[test]
    fn whichever_peer_speaks_first_is_taken() {
        let (serving, address) = serving();
        let mut quiet = Peer::to(&address);
        let mut loud = Peer::to(&address);
        quiet.say("hello");
        assert_eq!(next(&serving).expect("quiet").0, "hello");
        loud.say("loud");
        assert_eq!(next(&serving).expect("loud").0, "loud");
        assert_eq!(serving.open(), 2, "both kept");
        quiet.say("again");
        assert_eq!(next(&serving).expect("quiet again").0, "again");
    }

    #[test]
    fn a_last_exchange_and_a_hang_up_let_the_connection_go() {
        let (serving, address) = serving();
        let mut leaving = Peer::to(&address);
        leaving.say("bye");
        assert_eq!(next(&serving).expect("bye").0, "bye");
        assert_eq!(serving.open(), 0);
        let hanging_up = Peer::to(&address);
        let mut staying = Peer::to(&address);
        drop(hanging_up);
        staying.say("still here");
        assert_eq!(next(&serving).expect("staying").0, "still here");
        assert_eq!(serving.open(), 1, "the one that hung up is gone");
    }

    #[test]
    fn past_the_most_kept_the_one_quiet_longest_is_let_go() {
        let (serving, address) = serving();
        let mut first = Peer::to(&address);
        first.say("first");
        assert_eq!(next(&serving).expect("first").0, "first");
        let quiet: Vec<Peer> = (1..MOST_KEPT).map(|_| Peer::to(&address)).collect();
        let mut last = Peer::to(&address);
        last.say("last");
        assert_eq!(next(&serving).expect("last").0, "last");
        assert_eq!(serving.open(), MOST_KEPT);
        assert_eq!(first.heard(), "ok\n");
        assert_eq!(first.heard(), "", "let go: the first spoke longest ago");
        drop(quiet);
    }

    #[test]
    fn nothing_arriving_is_retryable_within_the_timeout() {
        let (serving, _) = serving();
        let began = Instant::now();
        let unbound = || -> Result<(TcpListener, String)> { panic!("bound twice") };
        let within = Some(Duration::from_millis(150));
        let error = serving
            .next(unbound, within, opened, line)
            .expect_err("nothing");
        assert!(error.retryable, "{error}");
        let waited = began.elapsed();
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
        assert!(waited < Duration::from_secs(2), "{waited:?}");
    }

    #[test]
    fn exchanges_on_a_kept_connection_take_under_a_millisecond_each() {
        // The owner's rule, held: around a payload, Xmip's own work is
        // under a millisecond. A thousand lines on one kept connection, each
        // answered before the next is said.
        const ROUNDS: u32 = 1000;
        let (serving, address) = serving();
        let peer = std::thread::spawn(move || {
            let mut peer = Peer::to(&address);
            for _ in 0..ROUNDS {
                peer.say("x");
                assert_eq!(peer.heard(), "ok\n");
            }
        });
        let began = Instant::now();
        for _ in 0..ROUNDS {
            next(&serving).expect("taken");
        }
        let took = began.elapsed();
        peer.join().expect("peer");
        assert!(
            took < Duration::from_millis(u64::from(ROUNDS)),
            "{ROUNDS} exchanges took {took:?}"
        );
    }
}
