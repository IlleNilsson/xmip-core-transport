//! A caller's turn on a Receive Location, bounded whole: every read of the
//! turn waits only what is left of one span from when the turn began (open
//! problem 69; ADR-0018, amendment 2026-11-14).
//!
//! A Receive Location takes its callers' turns one after another on one
//! thread ([`crate::serving`]), and until 2026-11-14 only each read of a
//! turn was bounded — by the Location's `timeout`, or not at all where it
//! sets none. A caller that sent half a request head and stopped held the
//! turn for as long as a read waits, for good where it waits as long as it
//! takes, and one that sent a byte inside each read's wait held it as long
//! as it liked; every other caller waited behind it.
//!
//! Now the serving wait hands every connection it accepts to its technology
//! as a [`Within`], and gives it a deadline before each turn: the turn's
//! span from now, the node's `[tuning] receive_turn`, a minute by default
//! ([`crate::serving::TURN`]). A read of the turn waits at most what is left
//! of it, where that is less than the Location's own wait, and a read once
//! the deadline passed, or once a read was cut at it, answers `TimedOut`
//! without reading: the turn ends as
//! any failure of the caller's own does, the caller turned away and the
//! others served ([`crate::turned`]). The Location's own wait still bounds
//! each read; the turn bounds them all together. Between turns the socket
//! waits as the Location says.
//!
//! The cost is one `setsockopt` for each read whose wait the turn shortens
//! — none where the Location's own wait ends a read before the turn would —
//! and one to put the Location's wait back once it was shortened.

use std::io::{self, BufReader, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use xcore::lock::Locked;

use crate::error::{Result, classify};

/// A connection a Receive Location serves: read within the turn its serving
/// wait gives it, and as its socket says between turns.
#[derive(Debug)]
pub struct Within {
    stream: TcpStream,
    deadline: Deadline,
}

/// When the turn a connection is in ends: given by the serving wait before
/// each turn and taken back after it, and read by every read of the
/// connection, wherever in a technology's readers the connection sits.
#[derive(Clone, Debug, Default)]
pub(crate) struct Deadline(Arc<Mutex<Turning>>);

#[derive(Debug, Default)]
struct Turning {
    /// When the turn ends, and the span it was given: `None` between turns.
    until: Option<(Instant, Duration)>,
    /// The Location's own wait on a read; `None` waits as long as it takes.
    wait: Option<Duration>,
    /// Whether the socket waits less than the Location's own wait now: the
    /// last read was given what was left of its turn.
    shortened: bool,
}

impl Deadline {
    /// A turn of `span` from now, each of its reads waiting at most `wait`,
    /// the Location's own.
    pub(crate) fn begin(&self, span: Duration, wait: Option<Duration>) {
        let mut turning = self.0.locked();
        turning.until = Some((Instant::now() + span, span));
        turning.wait = wait;
    }

    /// The turn over: a read waits as the Location says again.
    pub(crate) fn end(&self) {
        self.0.locked().until = None;
    }
}

impl Within {
    /// `stream`, read as its socket says until a serving wait gives it a
    /// turn: what [`crate::serving::Serving`] hands every connection it
    /// accepts, and what a technology accepting one connection alone, as a
    /// far end does, reads it through.
    #[must_use]
    pub fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            deadline: Deadline::default(),
        }
    }

    /// The socket: its readiness, its peer, a clone an answer is written on.
    #[must_use]
    pub const fn socket(&self) -> &TcpStream {
        &self.stream
    }

    /// The deadline its serving wait gives it.
    pub(crate) const fn deadline(&self) -> &Deadline {
        &self.deadline
    }

    /// This connection as a buffered reader, read within its turn, and a
    /// writer on a clone of its socket, as `socket::split` splits one.
    ///
    /// # Errors
    /// Where the socket could not be cloned.
    pub fn split(self) -> Result<(BufReader<Self>, TcpStream)> {
        let writer = self
            .stream
            .try_clone()
            .map_err(|e| classify("cloning the connection", &e))?;
        Ok((BufReader::new(self), writer))
    }

    /// What waits on the connection, left there, read within the turn as
    /// any read is.
    ///
    /// # Errors
    /// As a read: the turn passed, or the socket failed.
    pub fn peek(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.bounded()?;
        self.stream.peek(buffer).map_err(|e| self.past(e))
    }

    /// The socket's next read given what it may wait: what is left of the
    /// turn where that ends it before the Location's own wait would, the
    /// Location's wait put back where a read before was given less, and
    /// nothing at all once the turn passed.
    fn bounded(&self) -> io::Result<()> {
        let mut turning = self.deadline.0.locked();
        let wait = turning.wait;
        let shortened = match turning.until {
            Some((until, span)) => {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(passed(span));
                }
                // Where the Location's own wait ends the read first, the
                // socket waits as it is.
                wait.is_none_or(|wait| left < wait).then_some(left)
            }
            None => None,
        };
        match shortened {
            Some(left) => self.stream.set_read_timeout(Some(left))?,
            None if turning.shortened => self.stream.set_read_timeout(wait)?,
            None => {}
        }
        turning.shortened = shortened.is_some();
        Ok(())
    }

    /// `error`, said as the turn's end where the read waited out what was
    /// left of its turn, and the turn ended there: the socket's clock may
    /// end a wait a few milliseconds before this one's, and a turn said to
    /// have passed reads nothing more.
    fn past(&self, error: io::Error) -> io::Error {
        if !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
            return error;
        }
        let mut turning = self.deadline.0.locked();
        match turning.until {
            Some((_, span)) if turning.shortened => {
                turning.until = Some((Instant::now(), span));
                passed(span)
            }
            _ => error,
        }
    }
}

/// A turn of `span` that passed.
fn passed(span: Duration) -> io::Error {
    io::Error::new(
        ErrorKind::TimedOut,
        format!("the caller's turn passed its {} ms", span.as_millis()),
    )
}

impl Read for Within {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.bounded()?;
        self.stream.read(buffer).map_err(|e| self.past(e))
    }
}

impl Write for Within {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::{accept_tcp, bind_tcp, connect_tcp};

    const WAIT: Option<Duration> = Some(Duration::from_secs(2));

    /// A connection accepted with `wait` on its reads, and its peer.
    fn connected(wait: Option<Duration>) -> (Within, TcpStream) {
        let (listener, address) = bind_tcp("127.0.0.1:0").expect("bound");
        let peer = connect_tcp(&address, WAIT).expect("connected");
        let (stream, _) = accept_tcp(&listener, wait).expect("accepted");
        (Within::new(stream), peer)
    }

    #[test]
    fn a_read_past_the_turn_is_cut_and_the_next_turn_waits_as_the_socket_says() {
        let (mut within, mut peer) = connected(WAIT);
        within.deadline().begin(Duration::from_millis(100), WAIT);
        let began = Instant::now();
        let mut buffer = [0u8; 8];
        let cut = within.read(&mut buffer).expect_err("nothing sent");
        let waited = began.elapsed();
        assert_eq!(cut.kind(), ErrorKind::TimedOut, "{cut}");
        assert!(cut.to_string().contains("turn"), "{cut}");
        assert!(waited >= Duration::from_millis(90), "{waited:?}");
        assert!(waited < Duration::from_millis(1500), "{waited:?}");
        // Past the deadline nothing is read, though something waits.
        peer.write_all(b"x").expect("sent");
        let again = within.read(&mut buffer).expect_err("the turn passed");
        assert_eq!(again.kind(), ErrorKind::TimedOut, "{again}");
        within.deadline().end();
        assert_eq!(within.read(&mut buffer).expect("read"), 1);
        assert_eq!(within.socket().read_timeout().expect("its wait"), WAIT);
    }

    #[test]
    fn a_read_that_waited_out_its_turn_ends_the_turn_though_the_socket_woke_early() {
        let (within, _peer) = connected(WAIT);
        within.deadline().begin(Duration::from_secs(1), WAIT);
        within.bounded().expect("a second of the turn is left");
        // The socket's wait ends, as Windows' may, before the turn's clock.
        let cut = within.past(io::Error::from(ErrorKind::TimedOut));
        assert!(cut.to_string().contains("turn"), "{cut}");
        let again = within.bounded().expect_err("the turn passed");
        assert_eq!(again.kind(), ErrorKind::TimedOut, "{again}");
    }

    #[test]
    fn a_turn_the_locations_wait_ends_first_leaves_the_socket_as_it_is() {
        let wait = Some(Duration::from_millis(300));
        let (mut within, mut peer) = connected(wait);
        within.deadline().begin(Duration::from_secs(60), wait);
        peer.write_all(b"x").expect("sent");
        let mut buffer = [0u8; 8];
        assert_eq!(within.read(&mut buffer).expect("read"), 1);
        assert_eq!(within.socket().read_timeout().expect("its wait"), wait);
        assert!(!within.deadline.0.locked().shortened);
    }

    #[test]
    fn a_location_that_waits_as_long_as_it_takes_is_waited_on_only_its_turn() {
        let (mut within, mut peer) = connected(None);
        within.deadline().begin(Duration::from_secs(60), None);
        peer.write_all(b"xy").expect("sent");
        let mut buffer = [0u8; 1];
        assert_eq!(within.peek(&mut buffer).expect("peeked"), 1);
        assert_eq!(within.read(&mut buffer).expect("read"), 1);
        let given = within.socket().read_timeout().expect("its wait");
        assert!(given.is_some_and(|given| given <= Duration::from_secs(60)));
        within.deadline().end();
        assert_eq!(within.read(&mut buffer).expect("read"), 1);
        assert_eq!(within.socket().read_timeout().expect("its wait"), None);
    }
}
