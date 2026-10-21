//! A connection read to its end, let go at once so that neither end waits
//! to close.
//!
//! A Stream a connection frames ends when its sender closes, so the sender
//! closes first, and the side that closes first keeps the connection
//! waiting to close for two minutes on Windows and one on Linux, its port
//! held all that time. Over raw TCP that was every Message, on both of a
//! node's sides: the runtime's tests left some 1,260 connections waiting a
//! run, and five runs back to back about 13,000, until a connect found no
//! port left (2026-10-20). Once the receiver has read the sender's close,
//! everything sent has been received and nothing is left to say on the
//! connection, so the receiver lets it go with a reset (`SO_LINGER` of
//! zero): the connection ends on both sides at once, neither waiting. A
//! sender that waits for the receiver's close, rather than closing, is told
//! of a reset where it was told of a close. A connection not read to its
//! end is closed as before.

use std::io::{self, Read};
use std::net::TcpStream;
use std::time::Duration;

/// A received connection, its reads passed through: once a read finds the
/// sender's close, the connection is let go with a reset when dropped.
pub struct Ended {
    stream: TcpStream,
}

impl Ended {
    /// `stream`, a connection a Stream arrived on.
    #[must_use]
    pub const fn new(stream: TcpStream) -> Self {
        Self { stream }
    }
}

impl Read for Ended {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.stream.read(buffer)?;
        if read == 0 && !buffer.is_empty() {
            // Where the option is refused the connection is closed as
            // before, waiting to close on the sender's side: nothing is lost.
            let _ = rustix::net::sockopt::set_socket_linger(&self.stream, Some(Duration::ZERO));
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn a_connection_read_to_its_end_is_let_go_at_once() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
        let address = listener.local_addr().expect("an address");
        let mut sender = TcpStream::connect(address).expect("connected");
        sender.write_all(b"order").expect("written");
        sender
            .shutdown(std::net::Shutdown::Write)
            .expect("the sender closes");
        let (received, _) = listener.accept().expect("accepted");
        let mut ended = Ended::new(received);
        let mut body = Vec::new();
        ended.read_to_end(&mut body).expect("read to its end");
        assert_eq!(body, b"order");
        drop(ended);
        // The sender, waiting on its side, is told of the reset: its side
        // never saw a close of the receiver's, so it never waits to close.
        let mut rest = [0u8; 1];
        let told = sender.read(&mut rest);
        assert!(
            matches!(&told, Err(e) if e.kind() == io::ErrorKind::ConnectionReset),
            "{told:?}"
        );
    }

    #[test]
    fn a_connection_not_read_to_its_end_is_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bound");
        let address = listener.local_addr().expect("an address");
        let mut sender = TcpStream::connect(address).expect("connected");
        let (received, _) = listener.accept().expect("accepted");
        drop(Ended::new(received));
        let mut rest = [0u8; 1];
        assert_eq!(sender.read(&mut rest).expect("a close"), 0);
    }
}
