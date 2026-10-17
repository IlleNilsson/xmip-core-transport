//! What the machine itself takes beside a round: a plain loopback TCP wake
//! measured under the same load.
//!
//! The owner, 2026-09-26: what Xmip does around a payload is measured in
//! milliseconds, *apart from load*. A transport's latency test holds its
//! rounds, the fastest of each five ([`xcore::latency`]), to a bound plus
//! what [`TcpControl`] took — a thread woken by one byte on a loopback
//! connection, the least any TCP transport can do — so a machine busy
//! compiling fails nothing, and a transport that adds its own milliseconds
//! does. Compiled for tests only, as [`crate::payload`].

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use xcore::latency::{Spread, spread};

/// A plain loopback TCP connection with a thread waiting on its far side:
/// each [`TcpControl::poke`] is one byte written and the wake it causes
/// measured.
pub struct TcpControl {
    near: TcpStream,
    sent: Arc<Mutex<Vec<Instant>>>,
    waiter: thread::JoinHandle<Vec<Duration>>,
}

impl TcpControl {
    /// A connection on loopback, its far side waiting.
    ///
    /// # Panics
    /// Where loopback TCP is not available, which a test cannot do without.
    #[must_use]
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let address = listener.local_addr().expect("its address");
        let within = Some(Duration::from_secs(5));
        let near = crate::socket::connect_tcp(&address.to_string(), within).expect("connected");
        near.set_nodelay(true).expect("no delay");
        let (mut far, _) = crate::socket::accept_tcp(&listener, within).expect("accepted");
        let sent: Arc<Mutex<Vec<Instant>>> = Arc::default();
        let stamps = Arc::clone(&sent);
        let waiter = thread::spawn(move || {
            let mut taken = Vec::new();
            let mut byte = [0u8; 1];
            while far.read_exact(&mut byte).is_ok() {
                let now = Instant::now();
                let stamps = stamps.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(sent) = stamps.get(taken.len()) {
                    taken.push(now - *sent);
                }
            }
            taken
        });
        Self { near, sent, waiter }
    }

    /// One byte to the waiting side, stamped as it goes.
    ///
    /// # Panics
    /// Where the loopback connection broke.
    pub fn poke(&mut self) {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Instant::now());
        self.near.write_all(&[1]).expect("the control waits");
    }

    /// What the machine took to wake the waiting side, over every poke.
    ///
    /// # Panics
    /// Where the waiting side panicked.
    #[must_use]
    pub fn load(self) -> Spread {
        drop(self.near);
        spread(
            "a plain loopback TCP wake beside it",
            self.waiter.join().expect("the control woke"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcore::latency::spin;

    #[test]
    fn the_control_measures_every_poke() {
        let mut control = TcpControl::start();
        for _ in 0..20 {
            control.poke();
            spin(Duration::from_micros(200));
        }
        assert_eq!(control.load().rounds, 20);
    }
}
