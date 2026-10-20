//! What a send delivers: a Message's bytes, its length known before the
//! first of them, read from its start as often as a send needs, a piece at
//! a time; the delivery's deduplication key; and the Message's context,
//! which a technology that names what it writes reads the name from — a
//! file Send Location the name the file was received under
//! (`context::property::FILE_NAME`; ADR-0024, amendment 2026-10-20).
//!
//! The runtime hands a transport the Message as the Ledger keeps it, a
//! chunk at a time (`runtime-model.md` section 10), never read whole before
//! the send. A technology that sends as it reads writes the body on as it
//! comes ([`Outgoing::write_to`]); one whose protocol needs the whole body
//! first — a datagram, a frame or a record with its length or its digest
//! ahead of it, a signature over all of it — gathers it
//! ([`Outgoing::gather`]) and says so ([`crate::Sending`]). Either way the
//! body is held to its length: one that ends short of it, or runs on past
//! it, is refused in words, never sent as another body.
//!
//! **Read from its start as often as asked.** A kept session that fails on
//! reuse carries the exchange again on a new one (`crate::pool`), so a body
//! is opened afresh by every [`Outgoing::reader`]: from the Ledger again,
//! or from the bytes held, never from what an earlier attempt left.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read, Write};

use context::MessageContext;

use crate::error::{Result, TransportError, classify};

/// The bytes a streamed body is written on in, as the standard library's
/// own copy does.
const PIECE: usize = 8 * 1024;

/// What opens a streamed body from its start.
type Open<'a> = Box<dyn Fn() -> io::Result<Box<dyn Read + 'a>> + 'a>;

/// One send's body and key.
pub struct Outgoing<'a> {
    length: u64,
    body: Body<'a>,
    key: Option<&'a str>,
    /// Asked before the first byte goes: whether the send is still the
    /// sender's to make.
    still: Option<&'a dyn Fn() -> bool>,
    /// The Message's context, where the sender is the runtime.
    context: Option<&'a MessageContext>,
    /// Waited on before the far end may see the Message: until it is
    /// durable where its sender keeps it.
    kept: Option<&'a dyn Fn() -> Result<()>>,
}

enum Body<'a> {
    Held(&'a [u8]),
    Opened(Open<'a>),
}

impl<'a> Outgoing<'a> {
    /// `length` bytes, read from their start by every call of `open`.
    #[must_use]
    pub fn new(length: u64, open: impl Fn() -> io::Result<Box<dyn Read + 'a>> + 'a) -> Self {
        Self {
            length,
            body: Body::Opened(Box::new(open)),
            key: None,
            still: None,
            context: None,
            kept: None,
        }
    }

    /// Bytes held in memory: a test's, a tool's.
    #[must_use]
    pub const fn held(bytes: &'a [u8]) -> Self {
        Self {
            length: bytes.len() as u64,
            body: Body::Held(bytes),
            key: None,
            still: None,
            context: None,
            kept: None,
        }
    }

    /// The Message's, carrying its `context`: what the runtime hands every
    /// send, so a technology that names what it writes reads the name the
    /// Message carries.
    #[must_use]
    pub const fn carrying(mut self, context: &'a MessageContext) -> Self {
        self.context = Some(context);
        self
    }

    /// The Message's context, where the sender handed one: the runtime
    /// always does, a test or a tool sending held bytes does not.
    #[must_use]
    pub const fn context(&self) -> Option<&'a MessageContext> {
        self.context
    }

    /// Delivered under `key`: the delivery's deduplication key, the
    /// Journey's identifier, the same on every attempt of one Journey and
    /// different for every other (runtime-model section 15, *Delivery
    /// semantics*).
    #[must_use]
    pub const fn keyed(mut self, key: &'a str) -> Self {
        self.key = Some(key);
        self
    }

    /// Sent only while `still` says so when its first byte would go: once
    /// the body is gathered whole, or before the first piece is written on.
    /// Where it says no — the send's claim lost while the body was read —
    /// nothing is sent, and the send fails saying why. A send already under
    /// way is not called back: its key lets the far end deduplicate it
    /// (runtime-model section 10, *Corrected 2026-10-06*).
    #[must_use]
    pub const fn checked(mut self, still: &'a dyn Fn() -> bool) -> Self {
        self.still = Some(still);
        self
    }

    /// Let its far end see it only once `kept` returns: the runtime hands a
    /// technology that holds its delivery back ([`crate::Transport::holds_back`])
    /// a Message whose Publication is still being synced to the Ledger, so the
    /// send is written meanwhile and made visible once the Message is
    /// durable (ADR-0018, amendment 2026-10-20).
    #[must_use]
    pub const fn kept_by(mut self, kept: &'a dyn Fn() -> Result<()>) -> Self {
        self.kept = Some(kept);
        self
    }

    /// Wait until the Message is durable where its sender keeps it: at once
    /// where it is, or where the sender said nothing of it.
    ///
    /// # Errors
    ///
    /// Where it could not be kept: the far end must not see it, and the
    /// send fails retryable, its Journey kept or its sender told to send
    /// again.
    pub fn kept(&self) -> Result<()> {
        self.kept.map_or(Ok(()), |kept| kept())
    }

    /// Its length in bytes, known before any is read.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Its length, as this machine counts bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(self.length).unwrap_or(usize::MAX)
    }

    /// Whether it carries no bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The key it is delivered under, where it has one. A technology whose
    /// protocol carries an identifier its far end deduplicates by puts it
    /// there, and its README says so; every other sends at least once.
    #[must_use]
    pub const fn key(&self) -> Option<&'a str> {
        self.key
    }

    /// The body from its start, a piece at a time, held to its length.
    ///
    /// # Errors
    /// Where it cannot be opened, judged as [`Outgoing::write_to`] judges
    /// a read.
    pub fn reader(&self) -> Result<Reader<'_>> {
        let body: Box<dyn Read + '_> = match &self.body {
            Body::Held(bytes) => Box::new(*bytes),
            Body::Opened(open) => open().map_err(|failed| unread(&failed))?,
        };
        Ok(Reader {
            body,
            length: self.length,
            read: 0,
        })
    }

    /// The body whole, in memory: what a technology whose protocol needs it
    /// all before it sends asks for, and says so ([`crate::Sending`]). Held
    /// bytes are lent as they are.
    ///
    /// # Errors
    /// Where it cannot be read, or is not of its length.
    pub fn gather(&self) -> Result<Cow<'_, [u8]>> {
        if let Body::Held(bytes) = self.body {
            self.going()?;
            return Ok(Cow::Borrowed(bytes));
        }
        let mut whole = Vec::with_capacity(self.len());
        self.reader()?
            .read_to_end(&mut whole)
            .map_err(|failed| unread(&failed))?;
        self.going()?;
        Ok(Cow::Owned(whole))
    }

    /// Whether the first byte may go, as [`Outgoing::checked`] asks.
    fn going(&self) -> Result<()> {
        match self.still {
            Some(still) if !still() => Err(TransportError::retryable(
                "nothing was sent: the send was no longer the sender's once its Message was read",
            )),
            _ => Ok(()),
        }
    }

    /// The body written to `sink` as it is read, a piece at a time, never
    /// whole: what a technology that sends as it reads asks for. How many
    /// bytes were written, its length.
    ///
    /// # Errors
    /// Where the body cannot be read or is not of its length — permanent
    /// where its bytes are not the ones kept, retryable where they could
    /// not be reached — or `sink` refused a write, judged as [`classify`]
    /// judges it.
    pub fn write_to<W: Write + ?Sized>(&self, sink: &mut W) -> Result<u64> {
        let written = |failed: &io::Error| classify("writing the body", failed);
        if let Body::Held(bytes) = self.body {
            self.going()?;
            sink.write_all(bytes).map_err(|failed| written(&failed))?;
            return Ok(self.length);
        }
        self.pieces(PIECE, |piece| {
            sink.write_all(piece).map_err(|failed| written(&failed))
        })
    }

    /// The body handed to `each` a piece of `size` bytes at a time as it is
    /// read — the last piece shorter, an empty body one empty piece — never
    /// whole: what a technology whose protocol writes in requests of its own
    /// size asks for, an SFTP, SMB or NFS `WRITE`. How many bytes were
    /// handed on, its length.
    ///
    /// # Errors
    /// Where the body cannot be read or is not of its length, judged as
    /// [`Outgoing::write_to`] judges a read, or `each` failed.
    pub fn pieces(&self, size: usize, mut each: impl FnMut(&[u8]) -> Result<()>) -> Result<u64> {
        let size = size.max(1);
        if let Body::Held(bytes) = self.body {
            self.going()?;
            if bytes.is_empty() {
                each(&[])?;
            }
            for piece in bytes.chunks(size) {
                each(piece)?;
            }
            return Ok(self.length);
        }
        let mut reader = self.reader()?;
        let mut piece = vec![0u8; size];
        let mut handed = 0u64;
        loop {
            let taken = fill(&mut reader, &mut piece)?;
            if handed == 0 {
                self.going()?;
            }
            if taken > 0 || handed == 0 {
                each(&piece[..taken])?;
            }
            handed += taken as u64;
            if taken < size {
                return Ok(handed);
            }
        }
    }
}

/// As much of `reader` as fills `piece`: less only at its end.
fn fill(reader: &mut Reader<'_>, piece: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < piece.len() {
        match reader.read(&mut piece[filled..]) {
            Ok(0) => break,
            Ok(taken) => filled += taken,
            Err(failed) if failed.kind() == io::ErrorKind::Interrupted => {}
            Err(failed) => return Err(unread(&failed)),
        }
    }
    Ok(filled)
}

impl fmt::Debug for Outgoing<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Outgoing")
            .field("length", &self.length)
            .field("key", &self.key)
            .field("checked", &self.still.is_some())
            .field("carrying", &self.context.is_some())
            .finish_non_exhaustive()
    }
}

/// A body read from its start, held to its length: it ends where the
/// length does, and says so in words where the body ends sooner or runs on.
pub struct Reader<'a> {
    body: Box<dyn Read + 'a>,
    length: u64,
    read: u64,
}

impl Read for Reader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let left = self.length - self.read;
        if left == 0 {
            // Asked past its length: the body must end here too.
            let mut probe = [0u8; 1];
            return match self.body.read(&mut probe)? {
                0 => Ok(0),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the body runs on past its {} bytes", self.length),
                )),
            };
        }
        let most = out.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let taken = self.body.read(&mut out[..most])?;
        if taken == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the body ended after {} of its {} bytes",
                    self.read, self.length
                ),
            ));
        }
        self.read += taken as u64;
        Ok(taken)
    }
}

/// A body's read that failed: permanent where its bytes are not the ones
/// kept, retryable where they could not be reached — Xmip Storage not
/// answering is no verdict on the Message.
fn unread(failed: &io::Error) -> TransportError {
    let message = format!("the Message could not be read: {failed}");
    if failed.kind() == io::ErrorKind::InvalidData {
        TransportError::permanent(message)
    } else {
        TransportError::retryable(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A body of `length` patterned bytes that counts how often it was
    /// opened.
    fn opened(length: usize, opens: &Cell<usize>) -> Outgoing<'_> {
        Outgoing::new(length as u64, move || {
            opens.set(opens.get() + 1);
            let bytes: Vec<u8> = (0..length)
                .map(|at| u8::try_from(at % 251).unwrap_or_default())
                .collect();
            Ok(Box::new(io::Cursor::new(bytes)))
        })
    }

    #[test]
    fn a_held_body_is_lent_as_it_is_and_keyed() {
        let outgoing = Outgoing::held(b"ISA*00*").keyed("J1");
        assert_eq!((outgoing.length(), outgoing.key()), (7, Some("J1")));
        assert!(matches!(
            outgoing.gather().expect("whole"),
            Cow::Borrowed(b"ISA*00*")
        ));
        let mut sink = Vec::new();
        assert_eq!(outgoing.write_to(&mut sink).expect("written"), 7);
        assert_eq!(sink, b"ISA*00*");
    }

    #[test]
    fn a_streamed_body_is_written_a_piece_at_a_time_and_opened_afresh_each_time() {
        let opens = Cell::new(0);
        let outgoing = opened(3 * PIECE + 17, &opens);
        let mut first = Vec::new();
        assert_eq!(
            outgoing.write_to(&mut first).expect("written"),
            3 * 8192 + 17
        );
        let mut again = Vec::new();
        outgoing.write_to(&mut again).expect("written again");
        assert_eq!(first, again, "from its start, both times");
        assert_eq!(outgoing.gather().expect("whole").as_ref(), first.as_slice());
        assert_eq!(opens.get(), 3);
    }

    #[test]
    fn pieces_are_of_the_size_asked_the_last_shorter_and_an_empty_body_one_empty_piece() {
        let opens = Cell::new(0);
        for (length, sizes) in [(10, vec![4, 4, 2]), (8, vec![4, 4]), (0, vec![0])] {
            let mut held = Vec::new();
            let bytes = vec![7u8; length];
            Outgoing::held(&bytes)
                .pieces(4, |piece| {
                    held.push(piece.len());
                    Ok(())
                })
                .expect("held");
            let mut read = Vec::new();
            opened(length, &opens)
                .pieces(4, |piece| {
                    read.push(piece.len());
                    Ok(())
                })
                .expect("read");
            assert_eq!((&held, &read), (&sizes, &sizes), "{length} bytes");
        }
    }

    #[test]
    fn a_body_shorter_or_longer_than_its_length_is_refused_for_good() {
        let short = Outgoing::new(9, || Ok(Box::new(&b"short"[..])));
        let refused = short.write_to(&mut Vec::new()).expect_err("short");
        assert!(!refused.retryable);
        assert!(refused.message.contains("after 5 of its 9"), "{refused}");
        let long = Outgoing::new(2, || Ok(Box::new(&b"longer"[..])));
        let refused = long.gather().expect_err("long");
        assert!(refused.message.contains("runs on past its 2"), "{refused}");
    }

    #[test]
    fn a_send_no_longer_the_senders_once_its_body_is_read_sends_nothing() {
        let opens = Cell::new(0);
        let gone = || false;
        let refused = opened(100, &opens)
            .checked(&gone)
            .gather()
            .expect_err("let go");
        assert!(refused.message.contains("nothing was sent"), "{refused}");
        let mut sink = Vec::new();
        assert!(
            opened(100, &opens)
                .checked(&gone)
                .write_to(&mut sink)
                .is_err()
        );
        assert!(
            Outgoing::held(b"ISA")
                .checked(&gone)
                .write_to(&mut sink)
                .is_err()
        );
        assert!(sink.is_empty(), "not a byte written");
        let still = || true;
        assert_eq!(
            opened(100, &opens)
                .checked(&still)
                .write_to(&mut sink)
                .expect("sent"),
            100
        );
    }

    #[test]
    fn a_body_that_cannot_be_reached_may_be_sent_again() {
        let unreached = Outgoing::new(4, || Err(io::Error::other("no Storage node answered")));
        let failed = unreached.write_to(&mut Vec::new()).expect_err("unreached");
        assert!(failed.retryable, "{failed}");
        assert!(
            failed.message.contains("no Storage node answered"),
            "{failed}"
        );
    }

    #[test]
    fn a_sink_that_refuses_is_judged_as_its_failure() {
        struct Reset;
        impl Write for Reset {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::ConnectionReset))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let opens = Cell::new(0);
        let failed = opened(10, &opens).write_to(&mut Reset).expect_err("reset");
        assert!(failed.retryable && failed.message.contains("writing the body"));
    }
}
