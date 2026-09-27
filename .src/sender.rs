//! Sending datagrams from a socket bound once per address family, and
//! keeping it: the UDP technologies' side of [`crate::socket`].

use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::{Result, TransportError, classify};

/// The datagram socket a transport sends from: bound the first time a
/// target of its address family is sent to, IPv4 or IPv6, and kept for
/// every send after, by this transport and every clone of it.
///
/// Nine UDP technologies bound a new IPv4 socket for every datagram until
/// 2026-09-27, so each send paid a bind and an ephemeral port, and an IPv6
/// target could not be reached at all.
#[derive(Clone, Debug, Default)]
pub struct Sender {
    v4: Arc<Mutex<Option<UdpSocket>>>,
    v6: Arc<Mutex<Option<UdpSocket>>>,
    broadcast: bool,
    bound: Arc<AtomicUsize>,
}

impl Sender {
    /// A sender with nothing bound yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Permitted to send to a broadcast address, as `SO_BROADCAST` says.
    #[must_use]
    pub fn broadcasting(mut self) -> Self {
        self.broadcast = true;
        self
    }

    /// Send `bytes` to `target`, `host:port`, as one datagram.
    ///
    /// # Errors
    /// Where `target` does not resolve, the socket cannot be bound, or the
    /// datagram could not be sent.
    pub fn send_to(&self, bytes: &[u8], target: &str) -> Result<()> {
        let peer = resolve(target)?;
        let held = self.socket(peer)?;
        let socket = held.as_ref().ok_or_else(|| unbound(peer))?;
        socket
            .send_to(bytes, peer)
            .map_err(|e| classify("sending the datagram", &e))?;
        Ok(())
    }

    /// Run `exchange` — a request and the answer to it — on the socket for
    /// `target`'s family, held for it alone, with nothing waiting on it
    /// from before: an answer that came after its own request gave up is
    /// not taken for the answer to the next. `exchange` is handed the
    /// address `target` resolved to.
    ///
    /// # Errors
    /// Where `target` does not resolve, the socket cannot be bound, or as
    /// `exchange` fails.
    pub fn exchange<T>(
        &self,
        target: &str,
        exchange: impl FnOnce(&UdpSocket, SocketAddr) -> Result<T>,
    ) -> Result<T> {
        let peer = resolve(target)?;
        let held = self.socket(peer)?;
        let socket = held.as_ref().ok_or_else(|| unbound(peer))?;
        drain(socket)?;
        exchange(socket, peer)
    }

    /// How many sockets this sender has bound: one per address family it
    /// has sent to, however many datagrams.
    #[must_use]
    pub fn bound(&self) -> usize {
        self.bound.load(Ordering::Relaxed)
    }

    /// The socket for `peer`'s family, bound now where it is not yet.
    fn socket(&self, peer: SocketAddr) -> Result<MutexGuard<'_, Option<UdpSocket>>> {
        let (kept, any) = if peer.is_ipv4() {
            (&self.v4, "0.0.0.0:0")
        } else {
            (&self.v6, "[::]:0")
        };
        let mut held = kept.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            let socket =
                UdpSocket::bind(any).map_err(|e| classify("binding the sending socket", &e))?;
            if self.broadcast {
                socket
                    .set_broadcast(true)
                    .map_err(|e| classify("enabling broadcast", &e))?;
            }
            self.bound.fetch_add(1, Ordering::Relaxed);
            *held = Some(socket);
        }
        Ok(held)
    }
}

/// The first address `target` resolves to.
fn resolve(target: &str) -> Result<SocketAddr> {
    target
        .to_socket_addrs()
        .map_err(|e| classify("resolving the peer", &e))?
        .next()
        .ok_or_else(|| {
            TransportError::permanent(format!("{target} names no address")).at("sending")
        })
}

fn unbound(peer: SocketAddr) -> TransportError {
    TransportError::permanent(format!("no socket was bound to send to {peer}"))
}

/// Everything waiting on `socket`, read and dropped: late answers, and on
/// Windows the port-unreachable a send before provoked. Bounded, so a
/// flood cannot hold a send.
fn drain(socket: &UdpSocket) -> Result<()> {
    const MOST: usize = 64;
    socket
        .set_nonblocking(true)
        .map_err(|e| classify("clearing the socket", &e))?;
    let mut buffer = [0u8; 1];
    for _ in 0..MOST {
        // A datagram longer than the buffer is an error on Windows, and
        // gone all the same; only an empty socket ends the reading.
        if let Err(error) = socket.recv_from(&mut buffer)
            && error.kind() == ErrorKind::WouldBlock
        {
            break;
        }
    }
    socket
        .set_nonblocking(false)
        .map_err(|e| classify("clearing the socket", &e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn receiver(at: &str) -> Option<(UdpSocket, String)> {
        let socket = UdpSocket::bind(at).ok()?;
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let address = socket.local_addr().expect("address").to_string();
        Some((socket, address))
    }

    #[test]
    fn a_hundred_datagrams_leave_from_one_socket_bound_once() {
        let (far, address) = receiver("127.0.0.1:0").expect("bind");
        let sender = Sender::new();
        let clone = sender.clone();
        let mut from = Vec::new();
        for n in 0..100u8 {
            let sending = if n % 2 == 0 { &sender } else { &clone };
            sending.send_to(&[n], &address).expect("sent");
            let mut buffer = [0u8; 4];
            let (read, peer) = far.recv_from(&mut buffer).expect("arrived");
            assert_eq!(&buffer[..read], &[n]);
            from.push(peer);
        }
        from.dedup();
        assert_eq!(from.len(), 1, "one source port for every datagram");
        assert_eq!(sender.bound(), 1);
    }

    #[test]
    fn an_ipv6_target_is_sent_to_from_a_socket_of_its_own_family() {
        // A machine with IPv6 switched off has no [::1] to receive on.
        let Some((far, address)) = receiver("[::1]:0") else {
            return;
        };
        let sender = Sender::new();
        sender.send_to(b"six", &address).expect("sent");
        let mut buffer = [0u8; 4];
        let (read, peer) = far.recv_from(&mut buffer).expect("arrived");
        assert_eq!((&buffer[..read], peer.is_ipv6()), (&b"six"[..], true));
        let (_, four) = receiver("127.0.0.1:0").expect("bind");
        sender.send_to(b"four", &four).expect("sent");
        assert_eq!(sender.bound(), 2, "one per family");
    }

    #[test]
    fn an_answer_that_came_late_is_not_taken_for_the_next_one() {
        let (far, address) = receiver("127.0.0.1:0").expect("bind");
        let sender = Sender::new();
        let ask = |question: &[u8]| {
            sender.exchange(&address, |socket, peer| {
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| classify("timeout", &e))?;
                socket
                    .send_to(question, peer)
                    .map_err(|e| classify("send", &e))?;
                let mut buffer = [0u8; 8];
                let read = socket.recv(&mut buffer).map_err(|e| classify("recv", &e))?;
                Ok(buffer[..read].to_vec())
            })
        };
        let answer = |extra: bool| {
            let mut buffer = [0u8; 8];
            let (read, peer) = far.recv_from(&mut buffer).expect("asked");
            if extra {
                far.send_to(b"late", peer).expect("late");
            }
            far.send_to(&buffer[..read], peer).expect("answered");
        };
        let asking = std::thread::scope(|scope| {
            let far_end = scope.spawn(|| answer(true));
            let first = ask(b"one");
            far_end.join().expect("far end");
            first
        });
        // The first answer read was "late"; "one" is still waiting.
        assert_eq!(asking.expect("first"), b"late");
        let second = std::thread::scope(|scope| {
            let far_end = scope.spawn(|| answer(false));
            let second = ask(b"two");
            far_end.join().expect("far end");
            second
        });
        assert_eq!(second.expect("second"), b"two");
    }
}
