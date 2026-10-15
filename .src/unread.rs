//! A peer that sends everything before it reads its answer: what proves a
//! far end that answers before reading everything ends its connection
//! through [`crate::socket::finish`]. Compiled for tests only, as
//! [`crate::payload`].
//!
//! Windows resets a socket closed with bytes unread, and the reset throws
//! away the answer still waiting unread at the peer: a DICOM SCU writing
//! its data set lost the C-STORE-RSP of a store refused before the data set
//! was read, one run in three (2026-10-14). A test that reads each answer
//! as it comes, or sends only what is answered, never sees that.

use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

/// Bytes sent after `head`, unread by a far end that answers `head`:
/// enough to fill the socket buffers on both sides several times over.
pub const UNREAD: usize = 4 << 20;

/// Connect to `address`, send `head` and then [`UNREAD`] bytes of `filler`,
/// shut the write side, and only then hand the connection to `answer` to
/// read what the far end said. A far end that ends through `finish` has
/// read and dropped the filler, and its answer waits whole; one that drops
/// its socket bare resets the connection, the rest of the sending fails,
/// and the answer is thrown away with it — which `answer` then shows.
///
/// # Panics
/// Where the far end could not be reached.
pub fn answered_over_unread<T>(
    address: &str,
    head: &[u8],
    filler: u8,
    answer: impl FnOnce(&mut TcpStream) -> T,
) -> T {
    let mut stream = crate::socket::connect_tcp(address, Some(Duration::from_secs(5)))
        .expect("connecting to the far end");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bounding the reads");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("bounding the writes");
    let chunk = vec![filler; 64 << 10];
    let mut sending = || -> std::io::Result<()> {
        stream.write_all(head)?;
        for _ in 0..UNREAD / chunk.len() {
            stream.write_all(&chunk)?;
        }
        Ok(())
    };
    // A reset fails the sending; what the answer then reads says so.
    drop(sending());
    drop(stream.shutdown(Shutdown::Write));
    answer(&mut stream)
}
