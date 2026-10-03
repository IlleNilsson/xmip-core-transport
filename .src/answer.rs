//! The answer a peer waits for after the receive cycle: on the connection
//! its request came on, or as a datagram back to it.
//!
//! A request–response protocol answers a Stream with the verdict
//! (`crate::Acknowledgement`): `250` or `451`, ACK or NACK, the S frame or
//! nothing. The request is read on the connection a receive keeps
//! ([`crate::serving`]); its answer is written later, from the runtime's
//! thread, on a clone of that connection the acknowledgement holds — an
//! [`Answer`]. Where the protocol answers one request at a time, the
//! connection is [`Busy`] until then, so it takes no next request
//! (`serving::Open::busy`). An answer let go without a verdict shuts the
//! connection: the peer is never left waiting on a request nobody will
//! answer, and sends it again. What the answer says is the technology's;
//! holding, writing and letting go are here. A datagram's answer is sent
//! from a clone of the socket it came on ([`Datagram`]).

use std::fmt;
use std::io::Write;
use std::net::{Shutdown, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Result, classify};

/// Whether a connection waits on this side: a request taken from it whose
/// [`Answer`] is not given yet. Shared by the connection and its answer.
#[derive(Clone, Debug, Default)]
pub struct Busy(Arc<AtomicBool>);

impl Busy {
    /// Not busy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an answer is owed on the connection now.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn set(&self, busy: bool) {
        self.0.store(busy, Ordering::Release);
    }
}

/// The answer one peer waits for on its connection, given once.
pub struct Answer {
    connection: TcpStream,
    answered: bool,
    busy: Option<Busy>,
}

impl Answer {
    /// Hold `connection` for its answer: a clone of it, so whoever reads
    /// the connection keeps it.
    ///
    /// # Errors
    /// Where the connection could not be cloned.
    pub fn held(connection: &TcpStream) -> Result<Self> {
        let clone = connection
            .try_clone()
            .map_err(|e| classify("holding the connection for the answer", &e))?;
        Ok(Self::owning(clone))
    }

    /// The connection itself, owned by its answer: a peer that sends one
    /// request on a connection of its own.
    #[must_use]
    pub const fn owning(connection: TcpStream) -> Self {
        Self {
            connection,
            answered: false,
            busy: None,
        }
    }

    /// The connection is `busy` until this is answered or let go.
    #[must_use]
    pub fn busy(mut self, busy: &Busy) -> Self {
        busy.set(true);
        self.busy = Some(busy.clone());
        self
    }

    /// Answer with `bytes`, written whole and flushed.
    ///
    /// # Errors
    /// Where the peer went away before the answer.
    pub fn write(self, bytes: &[u8]) -> Result<()> {
        self.with(|connection| {
            connection
                .write_all(bytes)
                .map_err(|e| classify("writing the answer", &e))?;
            connection
                .flush()
                .map_err(|e| classify("flushing the answer", &e))
        })
    }

    /// Answer by `write`, which writes on the connection as its protocol
    /// frames an answer; the connection is free after it, written or not.
    ///
    /// # Errors
    /// As `write`.
    pub fn with<T>(mut self, write: impl FnOnce(&mut TcpStream) -> Result<T>) -> Result<T> {
        let written = write(&mut self.connection);
        self.free();
        written
    }

    /// Close the connection unanswered, where that is the protocol's
    /// negative answer: the peer sends again what was not answered.
    ///
    /// # Errors
    /// Where the connection could not be shut.
    pub fn shut(mut self) -> Result<()> {
        let shut = self
            .connection
            .shutdown(Shutdown::Both)
            .map_err(|e| classify("closing the connection unanswered", &e));
        self.free();
        shut
    }

    /// Answered or shut: nothing is owed, and the connection is free.
    fn free(&mut self) {
        self.answered = true;
        if let Some(busy) = self.busy.take() {
            busy.set(false);
        }
    }
}

impl Drop for Answer {
    /// Let go unanswered: the connection is shut, so its peer is not left
    /// waiting, and sends again.
    fn drop(&mut self) {
        if !self.answered {
            drop(self.connection.flush());
            drop(self.connection.shutdown(Shutdown::Both));
            self.free();
        }
    }
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer")
            .field("busy", &self.busy.is_some())
            .finish_non_exhaustive()
    }
}

/// The answer a datagram's sender waits for, sent back to it from the
/// socket the datagram came on.
#[derive(Debug)]
pub struct Datagram<A> {
    socket: UdpSocket,
    peer: A,
}

impl<A: ToSocketAddrs> Datagram<A> {
    /// The answer to `peer`, sent from a clone of `socket`.
    ///
    /// # Errors
    /// Where the socket could not be cloned.
    pub fn to(socket: &UdpSocket, peer: A) -> Result<Self> {
        let socket = socket
            .try_clone()
            .map_err(|e| classify("holding the socket for the answer", &e))?;
        Ok(Self { socket, peer })
    }

    /// Send `bytes` to the peer as one datagram.
    ///
    /// # Errors
    /// Where the datagram could not be sent.
    pub fn send(&self, bytes: &[u8]) -> Result<()> {
        self.socket
            .send_to(bytes, &self.peer)
            .map(drop)
            .map_err(|e| classify("sending the answer", &e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket;
    use std::io::Read;
    use std::time::Duration;

    const WAIT: Option<Duration> = Some(Duration::from_secs(2));

    /// A connected pair: the peer's end and this side's.
    fn pair() -> (TcpStream, TcpStream) {
        let (listener, address) = socket::bind_tcp("127.0.0.1:0").expect("bound");
        let peer = socket::connect_tcp(&address, WAIT).expect("connected");
        let (this, _) = socket::accept_tcp(&listener, WAIT).expect("accepted");
        peer.set_read_timeout(WAIT).expect("timeout");
        (peer, this)
    }

    #[test]
    fn an_answer_is_written_on_the_held_connection_and_frees_it() {
        let (mut peer, this) = pair();
        let busy = Busy::new();
        let answer = Answer::held(&this).expect("held").busy(&busy);
        assert!(busy.is_busy());
        answer.write(b"250 ok\r\n").expect("answered");
        assert!(!busy.is_busy());
        let mut heard = [0u8; 8];
        peer.read_exact(&mut heard).expect("heard");
        assert_eq!(&heard, b"250 ok\r\n");
        drop(this);
        assert_eq!(peer.read(&mut heard).expect("closed"), 0);
    }

    #[test]
    fn an_answer_let_go_unanswered_shuts_the_connection_and_frees_it() {
        let (mut peer, this) = pair();
        let busy = Busy::new();
        let answer = Answer::held(&this).expect("held").busy(&busy);
        drop(answer);
        assert!(!busy.is_busy());
        assert_eq!(peer.read(&mut [0u8; 4]).expect("shut"), 0, "let go");
        assert_eq!(
            format!("{:?}", Answer::owning(this)),
            "Answer { busy: false, .. }"
        );
    }

    #[test]
    fn an_answer_shut_as_its_negative_answer_says_nothing() {
        let (mut peer, this) = pair();
        Answer::owning(this).shut().expect("shut");
        assert_eq!(peer.read(&mut [0u8; 4]).expect("shut"), 0);
    }

    #[test]
    fn a_failed_write_still_frees_the_connection() {
        let (_, this) = pair();
        let busy = Busy::new();
        let error = Answer::held(&this)
            .expect("held")
            .busy(&busy)
            .with(|_| -> Result<()> { Err(crate::error::protocol_error("too long")) })
            .expect_err("failed");
        assert_eq!(error.message, "too long");
        assert!(!busy.is_busy());
    }

    #[test]
    fn a_datagram_answer_goes_back_to_its_sender() {
        let receiving = std::net::UdpSocket::bind("127.0.0.1:0").expect("bound");
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").expect("bound");
        sender.set_read_timeout(WAIT).expect("timeout");
        let to = sender.local_addr().expect("address");
        Datagram::to(&receiving, to)
            .expect("held")
            .send(b"ACK")
            .expect("sent");
        let mut heard = [0u8; 8];
        let (read, from) = sender.recv_from(&mut heard).expect("heard");
        assert_eq!(&heard[..read], b"ACK");
        assert_eq!(from, receiving.local_addr().expect("address"));
        let named = Datagram::to(&receiving, to.to_string()).expect("held");
        named.send(b"again").expect("sent by name");
    }
}
