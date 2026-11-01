//! A protocol whose exchange is a line, served on a kept listener
//! ([`crate::serving`]), and the peer that speaks it: what the serving
//! tests and the module's timing program (`tests/measured`) both drive.
//! Compiled for tests only, as [`crate::payload`].

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::answer::{Answer, Busy};
use crate::error::{Result, classify};
use crate::serving::{Open, Serving, Turn};
use crate::socket;

/// How long a peer connects and a receive waits for.
pub const WAIT: Option<Duration> = Some(Duration::from_secs(2));

/// A protocol whose exchange is a line, answered with `ok`; `bye` is the
/// connection's last; `wait` is answered later, by whoever takes its answer
/// with [`owed`].
pub struct Lines {
    reader: BufReader<TcpStream>,
    peer: SocketAddr,
    /// A `wait` taken and not answered: the connection is busy.
    busy: Busy,
}

thread_local! {
    /// The answers a `wait` owes, taken on this thread.
    static OWED: RefCell<Vec<Answer>> = const { RefCell::new(Vec::new()) };
}

/// The answer the last `wait` taken on this thread owes.
///
/// # Panics
///
/// Where no `wait` was taken on this thread.
#[must_use]
pub fn owed() -> Answer {
    OWED.with(|answers| answers.borrow_mut().pop().expect("owed"))
}

impl Open for Lines {
    fn socket(&self) -> &TcpStream {
        self.reader.get_ref()
    }

    fn waiting(&self) -> bool {
        !self.reader.buffer().is_empty()
    }

    fn busy(&self) -> Option<&Busy> {
        Some(&self.busy)
    }
}

/// The next line on `lines`, answered unless it is `wait`.
///
/// # Errors
///
/// A read or an answer that failed.
pub fn line(lines: &mut Lines) -> Result<Turn<(String, SocketAddr)>> {
    let mut line = String::new();
    let read = lines
        .reader
        .read_line(&mut line)
        .map_err(|e| classify("reading", &e))?;
    if read == 0 {
        return Ok(Turn::Closed);
    }
    if line.trim_end() == "wait" {
        let owed = Answer::held(lines.reader.get_ref())?.busy(&lines.busy);
        OWED.with(|answers| answers.borrow_mut().push(owed));
        return Ok(Turn::Taken((line.trim_end().to_string(), lines.peer)));
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
///
/// # Errors
///
/// A peer the socket cannot name.
///
/// # Panics
///
/// Where the socket names another peer than the one accepted.
pub fn opened(stream: TcpStream, accepted: SocketAddr) -> Result<Lines> {
    let peer = stream
        .peer_addr()
        .map_err(|e| classify("reading the peer", &e))?;
    assert_eq!(peer, accepted);
    Ok(Lines {
        reader: BufReader::new(stream),
        peer,
        busy: Busy::new(),
    })
}

/// A peer speaking lines.
pub struct Peer {
    reader: BufReader<TcpStream>,
}

impl Peer {
    /// A peer connected to `address`.
    ///
    /// # Panics
    ///
    /// Where it cannot connect.
    #[must_use]
    pub fn to(address: &str) -> Self {
        let stream = socket::connect_tcp(address, WAIT).expect("connect");
        Self {
            reader: BufReader::new(stream),
        }
    }

    /// `line` said.
    ///
    /// # Panics
    ///
    /// Where it cannot be written.
    pub fn say(&mut self, line: &str) {
        let written = format!("{line}\n");
        self.reader
            .get_mut()
            .write_all(written.as_bytes())
            .expect("say");
    }

    /// The next line heard; empty where the connection was let go.
    ///
    /// # Panics
    ///
    /// Where it cannot be read.
    pub fn heard(&mut self) -> String {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("hear");
        line
    }
}

/// A kept listener serving lines, bound once on loopback, and its address.
///
/// # Panics
///
/// Where it binds other than once.
#[must_use]
pub fn serving() -> (Serving<Lines>, String) {
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

/// The next line any peer of `serving` says, within [`WAIT`].
///
/// # Errors
///
/// Nothing said within the wait, or a line that could not be read.
pub fn next(serving: &Serving<Lines>) -> Result<(String, SocketAddr)> {
    waited(serving, WAIT)
}

/// The next line any peer of `serving` says, within `within`; as long as
/// it takes where `None`.
///
/// # Errors
///
/// Nothing said within the wait, or a line that could not be read.
///
/// # Panics
///
/// Where `serving` was not bound yet.
pub fn waited(serving: &Serving<Lines>, within: Option<Duration>) -> Result<(String, SocketAddr)> {
    let unbound = || -> Result<(TcpListener, String)> { panic!("bound twice") };
    serving.next(unbound, within, opened, line)
}
