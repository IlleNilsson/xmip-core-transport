//! The sessions a transport keeps between exchanges: opened and logged in
//! on the first send to an address, or the first receive of a Receive
//! Location that connects out, taken for one exchange and put back after,
//! and replaced when the far end closed one meanwhile.
//!
//! One pool, whatever a session is: an HTTP connection, an MQTT client, an
//! AMQP channel, a Postgres connection. Until 2026-09-27 twenty broker, SQL,
//! file-share and plant technologies connected, logged in and closed for
//! every message they sent, and HTTP alone kept its connections, in a pool
//! of its own; until 2026-09-28 their receives did the same for every poll.
//! A subscription a receive keeps is drained by [`delivered`], which tells
//! a quiet one from a broken one.

use std::borrow::Borrow;
use std::collections::BTreeMap;
use std::fmt;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::{Result, protocol_error};

/// What a pool keeps: a session that can say whether it can carry another
/// exchange.
pub trait Pooled: Send {
    /// Whether this session can carry another exchange: asked when it is
    /// taken from the pool and when it is put back, so one whose answer said
    /// it is done, or whose far end hung up while it waited, is let go
    /// rather than tried. Cheap: no round trip. [`alive`] is the check a
    /// session over TCP makes.
    fn usable(&mut self) -> bool {
        true
    }
}

/// The sessions a transport keeps to the addresses it sends to, `K` the
/// key they are found by — an address, or an address and what the session
/// was opened with.
///
/// Shared by the transport, every clone of it and every client it hands
/// the pool to. A session is taken for one exchange and put back after, so
/// sends on several threads each have one and nothing waits on another's
/// answer. One that fails on reuse — the far end closed it, restarted, or
/// timed it out — is dropped, and the exchange goes again on a new one: at
/// least once, as every send in Xmip is. A send that is not idempotent can
/// therefore be carried twice where the first attempt reached the far end
/// and only its answer was lost.
pub struct Pool<S, K = String> {
    kept: Arc<Mutex<BTreeMap<K, Vec<S>>>>,
    opened: Arc<AtomicUsize>,
}

impl<S, K> Clone for Pool<S, K> {
    fn clone(&self) -> Self {
        Self {
            kept: Arc::clone(&self.kept),
            opened: Arc::clone(&self.opened),
        }
    }
}

impl<S, K> Default for Pool<S, K> {
    fn default() -> Self {
        Self {
            kept: Arc::new(Mutex::new(BTreeMap::new())),
            opened: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl<S, K> fmt::Debug for Pool<S, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pool")
            .field("opened", &self.opened.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<S: Pooled, K: Ord> Pool<S, K> {
    /// None kept yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `exchange` on a session kept for `key`, or on a new one `open`
    /// opens where none is kept or the kept one failed, and keep the
    /// session for the next while it is [`Pooled::usable`].
    ///
    /// # Errors
    /// Where no session could be opened, or `exchange` failed on a new one.
    /// A new session an exchange failed on is not kept.
    pub fn exchange<Q, T>(
        &self,
        key: &Q,
        open: impl FnOnce() -> Result<S>,
        mut exchange: impl FnMut(&mut S) -> Result<T>,
    ) -> Result<T>
    where
        K: Borrow<Q>,
        Q: Ord + ToOwned<Owned = K> + ?Sized,
    {
        if let Some(mut session) = self.take(key)
            && let Ok(done) = exchange(&mut session)
        {
            self.put_back(key, session);
            return Ok(done);
        }
        self.let_go_unusable();
        let mut session = open()?;
        self.opened.fetch_add(1, Ordering::Relaxed);
        let done = exchange(&mut session)?;
        self.put_back(key, session);
        Ok(done)
    }

    /// Run `act` on the session kept for `key`, never on a new one: what
    /// answers a delivery — an ack, a reject, a `PUBACK` — belongs to the
    /// session it came on. Where that session is gone, `gone` says what the
    /// far end does instead (it delivers again), and nothing is opened. A
    /// session `act` failed on is let go.
    ///
    /// # Errors
    /// Where no session is kept for `key`, or as `act`.
    pub fn kept<Q, T>(
        &self,
        key: &Q,
        gone: &str,
        act: impl FnOnce(&mut S) -> Result<T>,
    ) -> Result<T>
    where
        K: Borrow<Q>,
        Q: Ord + ToOwned<Owned = K> + ?Sized,
    {
        let mut session = self.take(key).ok_or_else(|| protocol_error(gone))?;
        let done = act(&mut session)?;
        self.put_back(key, session);
        Ok(done)
    }

    /// How many sessions this pool has opened: one per key while the far
    /// end keeps them, however many exchanges.
    #[must_use]
    pub fn opened(&self) -> usize {
        self.opened.load(Ordering::Relaxed)
    }

    fn all(&self) -> MutexGuard<'_, BTreeMap<K, Vec<S>>> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every kept session that can no longer carry an exchange let go, under
    /// every key, and a key left with none forgotten: asked before a new
    /// session is opened, so a pool holds no socket its far end has closed
    /// for longer than until the next one it opens. A key that is never
    /// asked for again — a far end at a port of its own, gone — would
    /// otherwise keep its session for good, and until 2026-09-29 the
    /// Playground's nodes kept one such socket a round, in `CLOSE_WAIT`,
    /// until the machine was out of ephemeral ports.
    fn let_go_unusable(&self) {
        self.all().retain(|_, kept| {
            kept.retain_mut(Pooled::usable);
            !kept.is_empty()
        });
    }

    /// A kept session for `key` that can still carry an exchange; the ones
    /// that cannot are dropped on the way.
    fn take<Q>(&self, key: &Q) -> Option<S>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        loop {
            let mut session = self.all().get_mut(key)?.pop()?;
            if session.usable() {
                return Some(session);
            }
        }
    }

    /// `session` kept for the next exchange under `key`, where it can carry
    /// one; the key is copied only the first time.
    fn put_back<Q>(&self, key: &Q, mut session: S)
    where
        K: Borrow<Q>,
        Q: Ord + ToOwned<Owned = K> + ?Sized,
    {
        if !session.usable() {
            return;
        }
        let mut all = self.all();
        match all.get_mut(key) {
            Some(kept) => kept.push(session),
            None => {
                all.insert(key.to_owned(), vec![session]);
            }
        }
    }
}

/// Whether the far end of `stream` still has it open: nothing to read yet,
/// or something waiting. A peer that closed or reset it reads as the end
/// at once, without waiting, which is what this looks for.
///
/// A write to a connection the far end closed can succeed — the bytes go
/// into the socket's buffer, and the reset only comes back after — so a
/// session that only writes cannot find this out by sending.
#[must_use]
pub fn alive(stream: &TcpStream) -> bool {
    waiting(stream).is_some()
}

/// Whether the far end of `stream` still has it open and has sent nothing
/// since it was last read: what an idle session whose far end speaks only
/// when asked — an HTTP/1.1 server — must be to carry the next exchange.
/// Anything waiting there is not an answer to it: a TLS `close_notify`, a
/// timeout's `408`.
#[must_use]
pub fn quiet(stream: &TcpStream) -> bool {
    waiting(stream) == Some(false)
}

/// What a look at `stream` finds without waiting: whether something waits
/// to be read, or `None` where the far end closed or reset it.
fn waiting(stream: &TcpStream) -> Option<bool> {
    stream.set_nonblocking(true).ok()?;
    let mut one = [0u8; 1];
    let waiting = match stream.peek(&mut one) {
        Ok(read) => (read > 0).then_some(true),
        Err(error) => (error.kind() == ErrorKind::WouldBlock).then_some(false),
    };
    stream.set_nonblocking(false).ok()?;
    waiting
}

/// What a kept subscription delivers until it goes quiet: `next` asked for
/// one delivery after another on `session`, until the far end closes
/// (`None`) or nothing comes within the session's timeout.
///
/// A timeout on a subscription the far end still holds is quiet, not a
/// failure: the session is kept, subscribed, for the next receive, and what
/// arrived meanwhile waits for it at the far end or in the socket. A
/// failure after something arrived hands over what did — the session is
/// let go by [`Pooled::usable`] when it is put back — rather than lose it.
///
/// # Errors
/// Where `next` failed before anything arrived and the session cannot
/// carry another exchange, or failed for good: [`Pool::exchange`] then
/// goes again on a new session where this one was kept.
pub fn delivered<S: Pooled, T>(
    session: &mut S,
    mut next: impl FnMut(&mut S) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let mut arrived = Vec::new();
    loop {
        match next(session) {
            Ok(Some(one)) => arrived.push(one),
            Ok(None) => return Ok(arrived),
            Err(error) if error.retryable && (!arrived.is_empty() || session.usable()) => {
                return Ok(arrived);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::TransportError;
    use crate::socket;
    use std::io::{Read, Write};
    use std::time::Duration;

    /// A session over TCP: usable while its far end has it open.
    struct Line(TcpStream);

    impl Pooled for Line {
        fn usable(&mut self) -> bool {
            alive(&self.0)
        }
    }

    fn echo(line: &mut Line, byte: u8) -> Result<u8> {
        let mut back = [0u8; 1];
        line.0
            .write_all(&[byte])
            .and_then(|()| line.0.read_exact(&mut back))
            .map_err(|e| crate::error::classify("echo", &e))?;
        Ok(back[0])
    }

    /// Echoes one byte at a time on each connection it accepts, closing each
    /// after `per` bytes; how many connections it accepted.
    fn echoing(connections: usize, per: usize) -> (String, std::thread::JoinHandle<usize>) {
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

    fn opener(address: &str) -> impl FnOnce() -> Result<Line> + '_ {
        move || {
            Ok(Line(socket::connect_tcp(
                address,
                Some(Duration::from_secs(5)),
            )?))
        }
    }

    #[test]
    fn a_thousand_exchanges_on_one_key_open_one_session() {
        const EXCHANGES: usize = 1000;
        let (address, far_end) = echoing(1, EXCHANGES);
        let pool: Pool<Line> = Pool::new();
        let shared = pool.clone();
        let began = std::time::Instant::now();
        for n in 0..EXCHANGES {
            let by = if n % 2 == 0 { &pool } else { &shared };
            let byte = u8::try_from(n % 256).expect("a byte");
            let back = by
                .exchange(&address, opener(&address), |line| echo(line, byte))
                .expect("echoed");
            assert_eq!(back, byte);
        }
        // Generous for a debug build under load: a millisecond an exchange.
        let took = began.elapsed();
        assert!(took < Duration::from_millis(EXCHANGES as u64), "{took:?}");
        assert_eq!(pool.opened(), 1);
        drop((pool, shared));
        assert_eq!(far_end.join().expect("far end"), 1);
    }

    #[test]
    fn sessions_to_far_ends_that_hung_up_are_closed_not_kept() {
        // The Playground, 2026-09-29: every round sent to a far end at a new
        // port, the far end hung up after its exchange, and the pool kept
        // the session under a key never asked for again — a socket in
        // CLOSE_WAIT a round, until the machine had no ephemeral port left.
        // A session is closed when this side's socket is: the far end then
        // reads the end of the stream.
        const ROUNDS: usize = 50;
        let pool: Pool<Line> = Pool::new();
        let mut far_ends = Vec::new();
        for round in 0..ROUNDS {
            let (listener, address) = socket::bind_tcp("127.0.0.1:0").expect("bind");
            let far_end = std::thread::spawn(move || {
                let (mut stream, _) =
                    socket::accept_tcp(&listener, Some(Duration::from_secs(5))).expect("accept");
                let mut one = [0u8; 1];
                stream.read_exact(&mut one).expect("read");
                stream.write_all(&one).expect("echoed");
                stream.shutdown(std::net::Shutdown::Write).expect("hung up");
                stream
            });
            let byte = u8::try_from(round).expect("a byte");
            let back = pool
                .exchange(&address, opener(&address), |line| echo(line, byte))
                .expect("echoed");
            assert_eq!(back, byte);
            far_ends.push(far_end.join().expect("far end"));
        }
        assert_eq!(pool.opened(), ROUNDS);
        // The last round or two may not have seen their hang-up yet.
        for (round, mut stream) in far_ends.into_iter().take(ROUNDS - 2).enumerate() {
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .expect("timeout");
            let mut rest = [0u8; 1];
            let read = stream.read(&mut rest);
            assert!(
                matches!(read, Ok(0)),
                "round {round}: the pool still holds the session ({read:?})"
            );
        }
    }

    #[test]
    fn a_session_the_far_end_closed_is_replaced_before_it_is_used() {
        // One echo a connection, then the far end closes it.
        let (address, far_end) = echoing(3, 1);
        let pool: Pool<Line> = Pool::new();
        for byte in 1..=3 {
            // Where the close has landed before the next take, `alive` lets
            // the session go; where it has not, the echo fails on it and
            // goes again. A new session each time, either way.
            let back = pool
                .exchange(&address, opener(&address), |line| echo(line, byte))
                .expect("echoed");
            assert_eq!(back, byte);
        }
        assert_eq!(pool.opened(), 3);
        far_end.join().expect("far end");
    }

    #[test]
    fn an_exchange_that_fails_on_a_kept_session_goes_again_on_a_new_one() {
        struct Once(bool);
        impl Pooled for Once {}
        let pool: Pool<Once, u8> = Pool::new();
        pool.exchange(&1, || Ok(Once(false)), |_| Ok(()))
            .expect("first");
        // The kept session fails, as one whose far end went away does.
        let mut tried = 0;
        pool.exchange(
            &1,
            || Ok(Once(true)),
            |session| {
                tried += 1;
                if session.0 {
                    Ok(())
                } else {
                    Err(TransportError::retryable("the far end went away"))
                }
            },
        )
        .expect("sent again");
        assert_eq!((tried, pool.opened()), (2, 2));
        // A new session that fails is not kept.
        let failed = pool.exchange(
            &2,
            || Ok(Once(false)),
            |_| Err::<(), _>(TransportError::permanent("refused")),
        );
        assert!(failed.is_err());
        assert!(pool.all().get(&2).is_none_or(Vec::is_empty));
    }

    #[test]
    fn an_answer_goes_on_the_kept_session_or_not_at_all() {
        struct Consumer(u8);
        impl Pooled for Consumer {}
        let pool: Pool<Consumer, u8> = Pool::new();
        let gone = "the consumer is closed; the broker delivers it again";
        let error = pool.kept(&1, gone, |_| Ok(())).expect_err("none kept");
        assert_eq!((error.message.as_str(), error.retryable), (gone, false));
        pool.exchange(&1, || Ok(Consumer(7)), |_| Ok(()))
            .expect("received");
        let acked = pool
            .kept(&1, gone, |consumer| Ok(consumer.0))
            .expect("acked");
        assert_eq!((acked, pool.opened()), (7, 1), "never a new session");
        let failed = pool.kept(&1, gone, |_| {
            Err::<(), _>(TransportError::retryable("reset"))
        });
        assert!(failed.expect_err("failed").retryable);
        assert!(
            pool.kept(&1, gone, |_| Ok(())).is_err(),
            "a failed one is let go"
        );
    }

    #[test]
    fn a_quiet_subscription_is_kept_and_a_broken_one_is_not() {
        /// Delivers what it holds, then waits out its timeout; open or not.
        struct Subscribed(Vec<u8>, bool);
        impl Pooled for Subscribed {
            fn usable(&mut self) -> bool {
                self.1
            }
        }
        let next = |session: &mut Subscribed| match session.0.pop() {
            Some(one) => Ok(Some(one)),
            None => Err(TransportError::retryable("timed out")),
        };
        let pool: Pool<Subscribed, u8> = Pool::new();
        for _ in 0..5 {
            let quiet = pool
                .exchange(
                    &1,
                    || Ok(Subscribed(vec![2, 1], true)),
                    |s| delivered(s, next),
                )
                .expect("quiet");
            assert!(quiet.len() <= 2);
        }
        assert_eq!(pool.opened(), 1, "a quiet subscription is kept");
        // Closed after one delivery: that one is handed over, not lost.
        let mut closing = Subscribed(vec![7], false);
        assert_eq!(delivered(&mut closing, next).expect("one"), [7]);
        // Closed before any: a failure, so the pool opens a new one.
        let broken = delivered(&mut Subscribed(Vec::new(), false), next);
        assert!(broken.expect_err("broken").retryable);
        let ended = delivered(&mut Subscribed(Vec::new(), false), |_| Ok(None::<u8>));
        assert!(ended.expect("the far end closed").is_empty());
    }
}
