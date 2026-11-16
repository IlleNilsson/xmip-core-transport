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
//!
//! **A turn is bounded whole** (2026-11-14). Every connection is handed to
//! its technology as a [`Within`], and each turn on it is given a deadline
//! of [`Serving::turn`] from when it began — [`TURN`] by default, the
//! node's `[tuning] receive_turn` — every read of it waiting only what is
//! left ([`crate::deadline`]). A caller that stops mid-exchange, or sends a
//! byte inside each read's wait, is turned away at the deadline and the
//! others are served; until then it held them all. The turn is counted on
//! the Clock the serving wait was handed, the node's
//! ([`Serving::timed_by`]; ADR-0068, amendment 2026-11-15).
//!
//! **The listener is bound at open** (the owner, 2026-11-16, *Bind at
//! open*). A technology binds it through [`Serving::bound`] in its
//! `Configured::listening`, as the runtime opens its Receive Location's
//! transport, and a receive's wait ([`Serving::next`]) takes the listener
//! that bound and binds nothing ([`crate::kept`]). Until 2026-11-16 the
//! first receive bound it, after the node had started.
//!
//! The wait itself — one readiness wait over the listener and every kept
//! connection, a new peer accepted and kept — is `serving/wait.rs`'s.

mod wait;

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use xcore::lock::Locked;
use xcore::{Clock, SystemClock};

use crate::answer::{Busy, Freed};
use crate::deadline::{Deadline, Within};
use crate::error::Result;
use crate::kept::Kept;
use crate::turned::Turned;
use wait::Wait;

/// The most connections kept open at once. One more accepted lets the one
/// kept longest go, so a crowd of idle peers costs a bounded number of
/// sockets; a peer let go connects again.
const MOST_KEPT: usize = 64;

/// The most one caller's turn takes, where the node's `[tuning]
/// receive_turn` does not say: a minute, enough for `net::MAX_BODY`, 64 MiB,
/// at about ten megabits a second — the assistant's drafting of 2026-11-10;
/// the span is the owner's to set.
pub const TURN: Duration = Duration::from_secs(60);

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

/// A connection kept open, the peer it was accepted from — the caller
/// turned away where its own turn fails — and the deadline its reads are
/// given each turn.
struct Connection<C> {
    open: C,
    peer: SocketAddr,
    deadline: Deadline,
}

/// A kept listener and the connections kept open on it.
pub struct Serving<C> {
    listener: Kept<TcpListener>,
    open: Mutex<Vec<Connection<C>>>,
    /// What a busy connection's answer tells, made with the listener.
    freed: OnceLock<Freed>,
    /// The callers turned away since the runtime last asked.
    turned: Mutex<Turned>,
    /// The most one caller's turn takes.
    turn: Duration,
    /// What a turn is counted on.
    clock: Arc<dyn Clock>,
}

impl<C: Open> Serving<C> {
    /// Nothing bound, nothing open, each turn taking at most [`TURN`] on the
    /// operating system's clock.
    #[must_use]
    pub fn new() -> Self {
        Self {
            listener: Kept::new(),
            open: Mutex::new(Vec::new()),
            freed: OnceLock::new(),
            turned: Mutex::new(Turned::new()),
            turn: TURN,
            clock: Arc::new(SystemClock),
        }
    }

    /// Each caller's turn counted on `clock`, the node's: what a
    /// technology's `Configured::timed_by` hands its serving wait.
    #[must_use]
    pub fn timed_by(mut self, clock: &Arc<dyn Clock>) -> Self {
        self.clock = Arc::clone(clock);
        self
    }

    /// Each caller's turn taking at most `turn`, every read of it waiting
    /// only what is left: the node's `[tuning] receive_turn`, handed in as
    /// the runtime builds the transport (`Configured::turning_within`).
    #[must_use]
    pub const fn turning_within(mut self, turn: Duration) -> Self {
        self.turn = turn;
        self
    }

    /// The most one caller's turn takes.
    #[must_use]
    pub const fn turn(&self) -> Duration {
        self.turn
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

    /// Where the listener is bound: `None` until it is.
    #[must_use]
    pub fn address(&self) -> Option<&str> {
        self.listener.address()
    }

    /// Bind the listener by `bind` now, where it is not yet, and say where
    /// it is: what a technology's `Configured::listening` calls as its
    /// Receive Location's transport is opened, so the node listens when it
    /// says it started.
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
    /// Location does), on the listener [`Serving::bound`] bound as the
    /// transport was opened: a new peer made a connection by `opened` from
    /// the [`Within`] its reads go through, and kept, and a turn taken on a
    /// connection by `exchange`. A connection's timeout on each read is
    /// `timeout` too, and a turn's reads together take at most
    /// [`Serving::turn`], so a peer that stops mid-exchange, or dribbles,
    /// is let go. A peer whose connection could not be readied or opened,
    /// or whose turn failed, is let go and turned away
    /// ([`Serving::turned_away`]), and the wait goes on.
    ///
    /// # Errors
    /// Where nothing is bound ([`Kept::listener`]), the listener could not
    /// be waited on or accepted from, or nothing arrived within `timeout`
    /// (retryable).
    pub fn next<T>(
        &self,
        timeout: Option<Duration>,
        mut opened: impl FnMut(Within, SocketAddr) -> Result<C>,
        mut exchange: impl FnMut(&mut C) -> Result<Turn<T>>,
    ) -> Result<T> {
        let listener = self.listener.listener()?;
        let freed = self.freed()?;
        let mut opened = |stream, peer| {
            let made = opened(stream, peer)?;
            if let Some(busy) = made.busy() {
                busy.telling(freed);
            }
            Ok(made)
        };
        let mut open = self.open.locked();
        let waiting = Wait::new(listener, freed, &self.turned, &self.clock, timeout);
        loop {
            // What is already read goes first: no socket will say so.
            let Some(at) = open
                .iter()
                .position(|c| !owing(&c.open) && c.open.waiting())
                .map_or_else(|| waiting.spoken(&mut open, &mut opened), |at| Ok(Some(at)))?
            else {
                continue;
            };
            let connection = &mut open[at];
            connection.deadline.begin(self.turn, timeout);
            let turned = exchange(&mut connection.open);
            connection.deadline.end();
            match turned {
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

impl<C: Open> Default for Serving<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// A copy of a transport binds and keeps its own, as [`Kept`] does.
/// Its connections are its own too, so what tells them freed is; its
/// turns are as long, on the same Clock.
impl<C: Open> Clone for Serving<C> {
    fn clone(&self) -> Self {
        Self::new().turning_within(self.turn).timed_by(&self.clock)
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
    use crate::error::TransportError;
    use crate::lines::{Lines, Peer, WAIT, line, next, opened, serving, waited};
    use std::time::Instant;

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
        let refusing = |stream: Within, peer: SocketAddr| -> Result<Lines> {
            if std::mem::take(&mut first) {
                return Err(TransportError::permanent("refused at the door"));
            }
            opened(stream, peer)
        };
        let (said, _) = serving
            .next(WAIT, refusing, line)
            .expect("the next caller served");
        assert_eq!(said, "fine");
        let turned = serving.turned_away();
        assert_eq!(turned.callers.len(), 1, "{turned:?}");
        assert_eq!(turned.callers[0].peer, from);
        drop(refused);
    }

    #[test]
    fn a_listener_that_cannot_bind_fails_its_open_and_a_receive_binds_nothing() {
        let serving = Serving::<Lines>::new();
        let unbindable = || Err(TransportError::permanent("not this machine's address"));
        let error = serving.bound(unbindable).expect_err("no listener");
        assert_eq!(error.message, "not this machine's address");
        // Until 2026-11-16 a receive bound the listener where nothing had.
        let error = serving
            .next(WAIT, opened, line)
            .expect_err("nothing listens");
        assert!(!error.retryable, "{error}");
        assert!(error.message.contains("listening"), "{error}");
        assert_eq!(serving.address(), None);
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

    /// A peer of `address` that says the first byte of `line`, then one
    /// more every twenty milliseconds, each inside a read's wait: the peer
    /// once all is said, none where it was let go first.
    fn dribbling(
        address: &str,
        line: &'static [u8],
    ) -> (SocketAddr, std::thread::JoinHandle<Option<Peer>>) {
        let mut peer = Peer::to(address);
        let from = peer.calling_from();
        peer.send(&line[..1]);
        let dribbled = std::thread::spawn(move || {
            for byte in &line[1..] {
                std::thread::sleep(Duration::from_millis(20));
                peer.sending(&[*byte]).ok()?;
            }
            Some(peer)
        });
        (from, dribbled)
    }

    #[test]
    fn a_caller_that_stops_partway_is_cut_at_its_turn_and_the_next_is_served() {
        // A hundred bytes twenty milliseconds apart, each inside the
        // Location's two-second wait on a read: two seconds of one turn.
        // Until 2026-11-14 it held every other caller that long.
        let turn = Duration::from_millis(200);
        let (serving, address) = serving();
        let serving = serving.turning_within(turn);
        let (from, dribbled) = dribbling(&address, &[b'h'; 100]);
        let mut next_caller = Peer::to(&address);
        next_caller.say("fine");
        let began = Instant::now();
        let (said, _) = next(&serving).expect("the next caller served");
        let took = began.elapsed();
        assert_eq!(said, "fine");
        assert!(
            took >= turn,
            "served after {took:?}, inside the other's turn"
        );
        assert!(took < Duration::from_millis(1500), "served after {took:?}");
        let turned = serving.turned_away();
        assert_eq!(turned.callers.len(), 1, "{turned:?}");
        assert_eq!(turned.callers[0].peer, from);
        assert!(turned.callers[0].why.message.contains("turn"), "{turned:?}");
        drop(dribbled.join().expect("the dribbling caller"));
    }

    #[test]
    fn a_caller_sending_steadily_within_its_turn_is_served() {
        let (serving, address) = serving();
        let serving = serving.turning_within(Duration::from_secs(2));
        let (from, dribbled) = dribbling(&address, b"steady\n");
        let (said, peer) = next(&serving).expect("served whole");
        assert_eq!((said.as_str(), peer), ("steady", from));
        assert!(serving.turned_away().is_empty());
        let mut steady = dribbled
            .join()
            .expect("the steady caller")
            .expect("all said");
        assert_eq!(steady.heard(), "ok\n");
        assert_eq!(serving.clone().turn(), Duration::from_secs(2), "kept");
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
