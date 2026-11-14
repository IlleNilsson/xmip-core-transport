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
//!
//! A busy connection is left out of the wait, and its answer, given on
//! another thread, frees it while a receive waits for the others: the
//! answer tells the wait so (`answer::Freed`), and the wait takes the connection
//! in again at once. Until 2026-10-21 nothing told it, and the next request
//! on a connection freed meanwhile waited out the Location's whole wait — a
//! tenth of a second a Message in the runtime's node test, and for good
//! where the Location waits as long as it takes.
//!
//! **A caller's own failure ends its connection, never the Location**
//! (2026-11-13). Where readying a new peer's connection, opening it or a
//! turn on it fails, that connection is let go, its caller kept as turned
//! away ([`crate::turned`]) for the runtime to audit, and the wait goes on
//! for the others. Only the listener's own failure — its bind, its wait,
//! its accept — is the receive's.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use xcore::lock::Locked;

use rustix::fd::AsFd;

use crate::answer::{Busy, Freed};
use crate::error::{Result, TransportError, classify};
use crate::kept::Kept;
use crate::socket;
use crate::turned::Turned;

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

    /// Whether the connection waits on this side, where its protocol
    /// answers one exchange at a time: an exchange taken from it is not
    /// answered yet while its [`Busy`] says so, its answer the verdict of a
    /// receive cycle still running (`crate::Acknowledgement`). A busy
    /// connection takes no next exchange and is not waited on, so what its
    /// peer says next waits in its socket until it is answered; the answer
    /// tells the wait it is free.
    fn busy(&self) -> Option<&Busy> {
        None
    }
}

/// Whether `open` waits on this side now.
fn owing(open: &impl Open) -> bool {
    open.busy().is_some_and(Busy::is_busy)
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

/// A connection kept open, and the peer it was accepted from: the caller
/// turned away where its own turn fails.
struct Connection<C> {
    open: C,
    peer: SocketAddr,
}

/// A kept listener and the connections kept open on it.
pub struct Serving<C> {
    listener: Kept<TcpListener>,
    open: Mutex<Vec<Connection<C>>>,
    /// What a busy connection's answer tells, made with the listener.
    freed: OnceLock<Freed>,
    /// The callers turned away since the runtime last asked.
    turned: Mutex<Turned>,
}

impl<C: Open> Serving<C> {
    /// Nothing bound, nothing open.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            listener: Kept::new(),
            open: Mutex::new(Vec::new()),
            freed: OnceLock::new(),
            turned: Mutex::new(Turned::new()),
        }
    }

    /// The callers turned away since this was last asked, taken: what a
    /// technology's `Transport::turned_away` hands the runtime.
    #[must_use]
    pub fn turned_away(&self) -> Turned {
        std::mem::take(&mut *self.turned.locked())
    }

    /// What a busy connection's answer tells, made on the first wait.
    fn freed(&self) -> Result<&Freed> {
        if let Some(freed) = self.freed.get() {
            return Ok(freed);
        }
        let made = Freed::new()?;
        Ok(self.freed.get_or_init(|| made))
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
        self.open.locked().len()
    }

    /// The next exchange, from whichever peer sends first within `timeout`
    /// (`None` waits as long as it takes, which is what a listening Receive
    /// Location does): the listener bound by `bind` where it is not yet, a
    /// new peer made a connection by `opened` and kept, and a turn taken on
    /// a connection by `exchange`. A connection's timeout on its reads is
    /// `timeout` too, so a peer that stops mid-exchange is let go. A peer
    /// whose connection could not be readied or opened, or whose turn
    /// failed, is let go and turned away ([`Serving::turned_away`]), and
    /// the wait goes on.
    ///
    /// # Errors
    /// Where the listener could not be bound, waited on or accepted from,
    /// or nothing arrived within `timeout` (retryable).
    pub fn next<T>(
        &self,
        bind: impl FnOnce() -> Result<(TcpListener, String)>,
        timeout: Option<Duration>,
        mut opened: impl FnMut(TcpStream, SocketAddr) -> Result<C>,
        mut exchange: impl FnMut(&mut C) -> Result<Turn<T>>,
    ) -> Result<T> {
        let listener = self.listener.bound(bind)?;
        let freed = self.freed()?;
        let mut opened = |stream, peer| {
            let made = opened(stream, peer)?;
            if let Some(busy) = made.busy() {
                busy.telling(freed);
            }
            Ok(made)
        };
        let mut open = self.open.locked();
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        let waiting = Waiting {
            listener,
            freed,
            turned: &self.turned,
            deadline,
            timeout,
        };
        loop {
            // What is already read goes first: no socket will say so.
            let Some(at) = open
                .iter()
                .position(|c| !owing(&c.open) && c.open.waiting())
                .map_or_else(|| waiting.spoken(&mut open, &mut opened), |at| Ok(Some(at)))?
            else {
                continue;
            };
            match exchange(&mut open[at].open) {
                Ok(Turn::Taken(taken)) => {
                    // The one served goes to the back: the next wait
                    // favors the others, and the one let go when too
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
                // The caller's own failure: its connection, not the wait.
                Err(why) => {
                    let gone = open.remove(at);
                    self.turned.locked().away(gone.peer, why);
                }
            }
        }
    }
}

/// One receive's wait: the listener, what a freed connection tells, where
/// a caller turned away is kept, and how long the receive may wait.
struct Waiting<'a> {
    listener: &'a TcpListener,
    freed: &'a Freed,
    turned: &'a Mutex<Turned>,
    deadline: Option<Instant>,
    timeout: Option<Duration>,
}

impl Waiting<'_> {
    /// Wait until the deadline for the listener or an open connection: the
    /// first connection that spoke, if one did. A new peer is accepted and
    /// kept whenever the listener is ready, so busy connections never keep
    /// one waiting; the one quiet longest is let go where too many are
    /// kept. A connection freed meanwhile ends the wait with none, so the
    /// next waits on it too. A new peer whose connection could not be
    /// readied or opened is turned away.
    fn spoken<C: Open>(
        &self,
        open: &mut Vec<Connection<C>>,
        opened: &mut impl FnMut(TcpStream, SocketAddr) -> Result<C>,
    ) -> Result<Option<usize>> {
        let left = self
            .deadline
            .map(|at| at.saturating_duration_since(Instant::now()));
        if left.is_some_and(|left| left.is_zero()) {
            return Err(nothing_within(self.timeout));
        }
        let ready = self.wait(open, left)?;
        if ready.freed {
            self.freed.heard();
        }
        let mut spoke = ready.open.iter().position(|spoke| *spoke);
        if ready.listener {
            let nobody = socket::NOTHING_CONNECTED;
            let (stream, peer) = socket::accept_within(self.listener, self.timeout, nobody)?;
            match socket::accepted(&stream, self.timeout).and_then(|()| opened(stream, peer)) {
                Ok(made) => {
                    if open.len() == MOST_KEPT {
                        open.remove(0);
                        spoke = spoke.and_then(|at| at.checked_sub(1));
                    }
                    open.push(Connection { open: made, peer });
                }
                Err(why) => self.turned.locked().away(peer, why),
            }
        }
        Ok(spoke)
    }

    /// Which of the listener, the freed connections' word and the open
    /// connections is ready.
    fn wait<C: Open>(&self, open: &[Connection<C>], within: Option<Duration>) -> Result<Ready> {
        // A busy connection is not waited on: it answers nothing until its
        // exchange is, so it is reported quiet.
        let idle: Vec<usize> = (0..open.len())
            .filter(|&at| !owing(&open[at].open))
            .collect();
        let mut sources = Vec::with_capacity(idle.len() + 2);
        sources.push(self.listener.as_fd());
        sources.push(self.freed.socket().as_fd());
        sources.extend(idle.iter().map(|&at| open[at].open.socket().as_fd()));
        loop {
            match socket::ready(&sources, within) {
                Ok(said) => {
                    let mut ready = Ready {
                        listener: said[0],
                        freed: said[1],
                        open: vec![false; open.len()],
                    };
                    for (&at, &spoke) in idle.iter().zip(&said[2..]) {
                        ready.open[at] = spoke;
                    }
                    return Ok(ready);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(classify("waiting for a peer", &error)),
            }
        }
    }
}

/// What one wait found ready.
struct Ready {
    listener: bool,
    freed: bool,
    open: Vec<bool>,
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
/// Its connections are its own too, so what tells them freed is.
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
    use crate::lines::{Lines, Peer, WAIT, line, next, opened, serving, waited};

    #[test]
    fn a_caller_whose_turn_fails_is_turned_away_and_the_next_is_served() {
        let (serving, address) = serving();
        let mut broken = Peer::to(&address);
        let from = broken.calling_from();
        // Not UTF-8: no line, and the same next time. Until 2026-11-13 the
        // receive failed with it, and the runtime stopped the Location.
        broken.send(&[0xff, b'\n']);
        // The good caller calls once the broken one was let go, so the
        // broken turn is the first the receive takes.
        let after = address.clone();
        let callers = std::thread::spawn(move || {
            let let_go = broken.heard();
            let mut good = Peer::to(&after);
            good.say("fine");
            (let_go, good.heard())
        });
        let (said, _) = next(&serving).expect("the next caller served");
        assert_eq!(said, "fine");
        assert_eq!(
            callers.join().expect("callers"),
            (String::new(), "ok\n".to_string())
        );
        let turned = serving.turned_away();
        assert_eq!(turned.callers.len(), 1, "{turned:?}");
        assert_eq!(turned.callers[0].peer, from);
        assert!(!turned.callers[0].why.retryable, "{turned:?}");
        assert_eq!(serving.open(), 1, "the good caller kept");
        assert!(serving.turned_away().is_empty(), "taken once");
    }

    #[test]
    fn a_caller_whose_connection_cannot_be_opened_is_turned_away() {
        let (serving, address) = serving();
        let refused = Peer::to(&address);
        let from = refused.calling_from();
        let mut good = Peer::to(&address);
        good.say("fine");
        let mut first = true;
        let refusing = |stream: TcpStream, peer: SocketAddr| -> Result<Lines> {
            if std::mem::take(&mut first) {
                return Err(TransportError::permanent("refused at the door"));
            }
            opened(stream, peer)
        };
        let unbound = || -> Result<(TcpListener, String)> { panic!("bound twice") };
        let (said, _) = serving
            .next(unbound, WAIT, refusing, line)
            .expect("the next caller served");
        assert_eq!(said, "fine");
        let turned = serving.turned_away();
        assert_eq!(turned.callers.len(), 1, "{turned:?}");
        assert_eq!(turned.callers[0].peer, from);
        drop(refused);
    }

    #[test]
    fn a_listener_that_cannot_bind_fails_the_receive() {
        let serving = Serving::<Lines>::new();
        let unbindable = || Err(TransportError::permanent("not this machine's address"));
        let error = serving
            .next(unbindable, WAIT, opened, line)
            .expect_err("no listener");
        assert!(!error.retryable, "{error}");
        assert_eq!(error.message, "not this machine's address");
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
    fn a_busy_connection_takes_nothing_until_it_is_answered() {
        let (serving, address) = serving();
        let mut waiting = Peer::to(&address);
        waiting.say("wait");
        assert_eq!(next(&serving).expect("taken").0, "wait");
        // Said on the busy connection: it waits in its socket.
        waiting.say("pipelined");
        let mut other = Peer::to(&address);
        other.say("other");
        assert_eq!(next(&serving).expect("the other").0, "other");
        let within = Some(Duration::from_millis(50));
        let error = waited(&serving, within).expect_err("only the busy one has spoken");
        assert!(error.retryable, "{error}");
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
        let within = Some(Duration::from_millis(150));
        let error = waited(&serving, within).expect_err("nothing");
        assert!(error.retryable, "{error}");
        let waited = began.elapsed();
        assert!(waited >= Duration::from_millis(100), "{waited:?}");
        assert!(waited < Duration::from_secs(2), "{waited:?}");
    }
}
