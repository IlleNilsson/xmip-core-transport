//! One Stream as it arrived: where it came from, its body as a reader the
//! runtime pulls in chunks, and how its far end is told the receive cycle
//! ended.

use std::fmt;
use std::io::{Cursor, Read};

use crate::acknowledgement::{Acknowledgement, Refusal, Verdict};
use crate::error::{Result, classify};
use crate::taken::Taken;

/// What a transport hands back from `receive`.
///
/// `origin_uri` is historical fact and never changes, per ADR-0013. It says
/// where the bytes came from, not where they are now — a file that is later
/// consumed by being moved still arrived from the path it was read at.
///
/// **The body is a reader, never a whole buffer where the far end can
/// stream** (the owner, 2026-10-01: *A stream can't be written completely
/// to memory*): the runtime pulls it in chunks into the Ledger. A file, an
/// FTP or SFTP transfer, an SMB or `WebDAV` read is read as the runtime asks;
/// a protocol that hands over a whole message — a queue message, a
/// datagram, a frame — gives a reader over it ([`Arrived::whole`]).
///
/// **Nothing is consumed before the acknowledgement.** The far end keeps
/// what arrived until the runtime gives the [`Acknowledgement`] its
/// [`Verdict`] after the whole receive cycle (runtime-model section 5).
pub struct Arrived {
    pub origin_uri: String,
    body: Box<dyn Read + Send>,
    acknowledgement: Acknowledgement,
}

impl Arrived {
    /// A Stream whose body is read from `body` as the runtime asks, and
    /// whose far end is told the verdict by `acknowledgement`.
    #[must_use]
    pub fn new(
        origin_uri: impl Into<String>,
        body: impl Read + Send + 'static,
        acknowledgement: Acknowledgement,
    ) -> Self {
        Self {
            origin_uri: origin_uri.into(),
            body: Box::new(body),
            acknowledgement,
        }
    }

    /// A Stream the protocol handed over whole — a queue message, a frame,
    /// a datagram — read through a reader over it like any other.
    #[must_use]
    pub fn whole(
        origin_uri: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        acknowledgement: Acknowledgement,
    ) -> Self {
        Self::new(origin_uri, Cursor::new(bytes.into()), acknowledgement)
    }

    /// Whether the far end waits for the verdict; `false` is an
    /// at-most-once protocol ([`Acknowledgement::at_most_once`]).
    #[must_use]
    pub const fn defers(&self) -> bool {
        self.acknowledgement.defers()
    }

    /// The origin, the body to pull and the acknowledgement to give once
    /// the cycle has ended: what the runtime takes apart.
    #[must_use]
    pub fn into_parts(self) -> (String, Box<dyn Read + Send>, Acknowledgement) {
        (self.origin_uri, self.body, self.acknowledgement)
    }

    /// Read the whole body and accept it: what a far end does with what it
    /// received, and a technology's test with what its receive found. A
    /// Receive Location never does — its runtime pulls the body in chunks
    /// and gives the verdict after the cycle.
    ///
    /// # Errors
    /// Where the body could not be read, which is a failed cycle, or the
    /// far end could not be told.
    pub fn taken(self) -> Result<Taken> {
        let (origin_uri, mut body, acknowledgement) = self.into_parts();
        let mut bytes = Vec::new();
        if let Err(error) = body.read_to_end(&mut bytes) {
            drop(body);
            acknowledgement.acknowledge(Verdict::Failed)?;
            return Err(classify("reading what arrived", &error));
        }
        drop(body);
        acknowledgement.acknowledge(Verdict::Accepted)?;
        Ok(Taken::new(origin_uri, bytes))
    }

    /// Refuse it unread, for `why`: the far end hears the protocol's
    /// permanent rejection and does not send it again. What a technology's
    /// test does to prove a refused cycle.
    ///
    /// # Errors
    /// Where the far end could not be told.
    pub fn refused(self, why: Refusal) -> Result<()> {
        self.told(Verdict::Refused(why))
    }

    /// Fail it unread: the far end keeps it and sends it again. What a
    /// technology's test does to prove a failed cycle consumes nothing.
    ///
    /// # Errors
    /// Where the far end could not be told.
    pub fn failed(self) -> Result<()> {
        self.told(Verdict::Failed)
    }

    fn told(self, verdict: Verdict) -> Result<()> {
        let (_, body, acknowledgement) = self.into_parts();
        drop(body);
        acknowledgement.acknowledge(verdict)
    }
}

impl fmt::Debug for Arrived {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Arrived")
            .field("origin_uri", &self.origin_uri)
            .field("acknowledgement", &self.acknowledgement)
            .finish_non_exhaustive()
    }
}

/// The first of `arrivals`, or the failure `missing` says when there is
/// none: what a far end hands back from a receive that may come up empty —
/// a session's next publish, the rows a read-back found. Nine far ends
/// wrote `.into_iter().next().ok_or_else(..)` until 2026-09-24.
///
/// # Errors
/// Where nothing arrived: a peer that broke the exchange, which saying
/// again will not mend.
pub fn next_arrival<T>(arrivals: impl IntoIterator<Item = T>, missing: &str) -> Result<T> {
    arrivals
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::protocol_error(missing))
}

/// The one of `arrivals`, where exactly one was due, or the failure that
/// says how many came instead: `"<did> <count> messages, not one"`, where
/// `did` is what brought them — `"collected"`, `"the client appended"`.
/// POP3's and IMAP's far ends each wrote it until 2026-09-24.
///
/// # Errors
/// Where none or more than one arrived.
pub fn one_arrival<T>(mut arrivals: Vec<T>, did: &str) -> Result<T> {
    match arrivals.len() {
        1 => Ok(arrivals.remove(0)),
        count => Err(crate::error::protocol_error(format!(
            "{did} {count} messages, not one"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// An acknowledgement that writes its verdict where the test reads it.
    fn told() -> (Arc<Mutex<Option<Verdict>>>, Acknowledgement) {
        let verdict = Arc::new(Mutex::new(None));
        let into = Arc::clone(&verdict);
        let acknowledgement = Acknowledgement::deferred(move |given| {
            *into.lock().expect("lock") = Some(given);
            Ok(())
        });
        (verdict, acknowledgement)
    }

    #[test]
    fn one_arrival_is_the_one_or_says_how_many_came() {
        assert_eq!(one_arrival(vec![1], "collected").expect("one"), 1);
        let error = one_arrival(Vec::<u8>::new(), "collected").expect_err("none");
        assert_eq!(error.message, "collected 0 messages, not one");
    }

    #[test]
    fn the_next_arrival_is_the_first_or_says_what_is_missing() {
        assert_eq!(
            next_arrival(["a://1", "a://2"], "none").expect("first"),
            "a://1"
        );
        let error = next_arrival(None::<u8>, "the client closed without one").expect_err("none");
        assert_eq!(error.message, "the client closed without one");
    }

    #[test]
    fn an_arrival_carries_its_origin_and_streams_its_body() {
        let (verdict, acknowledgement) = told();
        let arrived = Arrived::new("file:///in/order.edi", &b"ISA*00*"[..], acknowledgement);
        assert!(arrived.defers());
        let (origin, mut body, acknowledgement) = arrived.into_parts();
        assert_eq!(origin, "file:///in/order.edi");
        let mut chunk = [0u8; 4];
        body.read_exact(&mut chunk).expect("a chunk");
        assert_eq!(&chunk, b"ISA*");
        assert_eq!(*verdict.lock().expect("lock"), None, "nothing told yet");
        acknowledgement
            .acknowledge(Verdict::Accepted)
            .expect("told");
        assert_eq!(*verdict.lock().expect("lock"), Some(Verdict::Accepted));
    }

    #[test]
    fn taken_reads_it_whole_and_accepts_it_and_refused_reads_nothing() {
        let (verdict, acknowledgement) = told();
        let taken = Arrived::whole("q://one", b"row".to_vec(), acknowledgement)
            .taken()
            .expect("taken");
        assert_eq!(
            (taken.origin_uri.as_str(), taken.bytes.as_slice()),
            ("q://one", &b"row"[..])
        );
        assert_eq!(*verdict.lock().expect("lock"), Some(Verdict::Accepted));

        let (verdict, acknowledgement) = told();
        Arrived::whole("q://two", b"row".to_vec(), acknowledgement)
            .refused(Refusal::Unacceptable)
            .expect("refused");
        assert_eq!(
            *verdict.lock().expect("lock"),
            Some(Verdict::Refused(Refusal::Unacceptable))
        );

        let (verdict, acknowledgement) = told();
        Arrived::whole("q://three", b"row".to_vec(), acknowledgement)
            .failed()
            .expect("failed");
        assert_eq!(*verdict.lock().expect("lock"), Some(Verdict::Failed));
    }

    #[test]
    fn a_body_that_breaks_is_a_failed_cycle_not_accepted() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the share went away"))
            }
        }
        let (verdict, acknowledgement) = told();
        let error = Arrived::new("smb://one", Broken, acknowledgement)
            .taken()
            .expect_err("broken");
        assert!(error.message.contains("the share went away"), "{error}");
        assert_eq!(*verdict.lock().expect("lock"), Some(Verdict::Failed));
    }

    #[test]
    fn an_arrival_says_how_it_is_acknowledged() {
        let arrived = Arrived::whole(
            "udp://127.0.0.1:9",
            vec![1],
            Acknowledgement::at_most_once("a datagram has nobody to answer"),
        );
        assert!(!arrived.defers());
        assert_eq!(
            format!("{arrived:?}"),
            "Arrived { origin_uri: \"udp://127.0.0.1:9\", acknowledgement: \
             Acknowledgement::AtMostOnce(\"a datagram has nobody to answer\"), .. }"
        );
    }
}
