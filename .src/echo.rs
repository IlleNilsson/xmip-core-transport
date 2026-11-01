//! A far end echoing a byte at a time, and the pooled TCP session that asks
//! it ([`crate::pool`]): what the pool's tests and the module's timing
//! program (`tests/measured`) both drive. Compiled for tests only, as
//! [`crate::payload`].

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{Result, classify};
use crate::pool::{Pooled, alive};
use crate::socket;

/// A session over TCP: usable while its far end has it open.
pub struct Line(pub TcpStream);

impl Pooled for Line {
    fn usable(&mut self) -> bool {
        alive(&self.0)
    }
}

/// `byte` written on `line`, and what the far end echoed.
///
/// # Errors
///
/// A write or a read that failed.
pub fn echo(line: &mut Line, byte: u8) -> Result<u8> {
    let mut back = [0u8; 1];
    line.0
        .write_all(&[byte])
        .and_then(|()| line.0.read_exact(&mut back))
        .map_err(|e| classify("echo", &e))?;
    Ok(back[0])
}

/// A far end that echoes one byte at a time on each of `connections` it
/// accepts, closing each after `per` bytes: where it listens, and its
/// thread, which answers how many connections it accepted.
///
/// # Panics
///
/// Where it cannot bind, or a connection is not made within five seconds.
#[must_use]
pub fn echoing(connections: usize, per: usize) -> (String, JoinHandle<usize>) {
    let (listener, address) = socket::bind_tcp("127.0.0.1:0").expect("bind");
    let far_end = std::thread::spawn(move || {
        for _ in 0..connections {
            let (mut stream, _) =
                socket::accept_tcp(&listener, Some(Duration::from_secs(5))).expect("accept");
            let mut one = [0u8; 1];
            for _ in 0..per {
                if stream.read_exact(&mut one).is_err() {
                    break;
                }
                stream.write_all(&one).expect("echoed");
            }
        }
        connections
    });
    (address, far_end)
}

/// What opens a [`Line`] to `address`, as a pool asks it to.
pub fn opener(address: &str) -> impl FnOnce() -> Result<Line> + '_ {
    move || {
        Ok(Line(socket::connect_tcp(
            address,
            Some(Duration::from_secs(5)),
        )?))
    }
}
