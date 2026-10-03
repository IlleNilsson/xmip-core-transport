//! The bodies a transport hands the runtime where the far end streams: one
//! opened by its first read, and one pulled a chunk at a time.
//!
//! An arrival's body is read on the runtime's thread after `receive` has
//! returned, as the runtime asks ([`crate::Arrived`]). Nothing is fetched,
//! opened or transferred before then, so a receive that lists a hundred
//! files holds no hundred open handles, and an arrival dropped unread
//! costs the far end nothing. What a technology fetches, opens or reads
//! next is its own, handed in; when it is asked for is here.

use std::io::{self, Cursor, Read};

use crate::error::Result;

/// A body opened by its first read: `open` runs then — a file opened, a
/// transfer begun — and what it opened is read on.
pub struct Opened<F, R> {
    open: Option<F>,
    reader: Option<R>,
}

/// A body `open` opens when it is first read.
#[must_use]
pub const fn opened<F, R>(open: F) -> Opened<F, R>
where
    F: FnOnce() -> Result<R>,
    R: Read,
{
    Opened {
        open: Some(open),
        reader: None,
    }
}

/// A body the far end hands over whole, `fetch`ed by its first read: a
/// mail message, a locked resource's `GET`.
#[must_use]
pub fn fetched<F>(fetch: F) -> impl Read + Send + 'static
where
    F: FnOnce() -> Result<Vec<u8>> + Send + 'static,
{
    opened(move || fetch().map(Cursor::new))
}

impl<F, R> Read for Opened<F, R>
where
    F: FnOnce() -> Result<R>,
    R: Read,
{
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.reader.is_none() {
            // A failed open is said once; the body is not opened twice.
            let open = self
                .open
                .take()
                .ok_or_else(|| io::Error::other("the body could not be opened"))?;
            self.reader = Some(open().map_err(io::Error::other)?);
        }
        self.reader
            .as_mut()
            .map_or(Ok(0), |reader| reader.read(buffer))
    }
}

/// A body pulled a chunk at a time: `next` asked for one chunk after
/// another — an NFS or SMB `READ`, the next block off a channel — until it
/// says the end with `None`. The first is asked for by the first read.
pub struct Chunked<F> {
    next: F,
    chunk: Vec<u8>,
    at: usize,
    ended: bool,
}

/// A body `next` hands over a chunk at a time, `None` at its end. Whatever
/// `next` holds — an open file, a handle — is let go with the body.
#[must_use]
pub const fn chunked<F>(next: F) -> Chunked<F>
where
    F: FnMut() -> Result<Option<Vec<u8>>>,
{
    Chunked {
        next,
        chunk: Vec::new(),
        at: 0,
        ended: false,
    }
}

impl<F> Read for Chunked<F>
where
    F: FnMut() -> Result<Option<Vec<u8>>>,
{
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        // An empty chunk is not the end: only `None` says that.
        while self.at == self.chunk.len() {
            if self.ended {
                return Ok(0);
            }
            let next = (self.next)().map_err(io::Error::other)?;
            self.ended = next.is_none();
            self.chunk = next.unwrap_or_default();
            self.at = 0;
        }
        let n = buffer.len().min(self.chunk.len() - self.at);
        buffer[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::protocol_error;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_body_is_opened_by_its_first_read_and_once() {
        let opens = AtomicUsize::new(0);
        let mut body = opened(|| {
            opens.fetch_add(1, Ordering::SeqCst);
            Ok(&b"ISA*00*"[..])
        });
        assert_eq!(body.read(&mut []).expect("nothing asked"), 0);
        assert_eq!(opens.load(Ordering::SeqCst), 0, "not opened before a read");
        let mut read = Vec::new();
        body.read_to_end(&mut read).expect("read");
        assert_eq!(read, b"ISA*00*");
        assert_eq!(body.read(&mut [0; 4]).expect("the end"), 0);
        assert_eq!(opens.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_body_that_cannot_be_opened_says_so_every_read() {
        let mut body = opened(|| -> Result<&[u8]> { Err(protocol_error("no such file")) });
        let error = body.read(&mut [0; 4]).expect_err("not opened");
        assert!(error.to_string().contains("no such file"), "{error}");
        assert!(body.read(&mut [0; 4]).is_err(), "never read as empty");
    }

    #[test]
    fn a_fetched_body_is_fetched_whole_on_the_first_read() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&fetches);
        let mut body = fetched(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(b"From: r1".to_vec())
        });
        let mut first = [0u8; 4];
        body.read_exact(&mut first).expect("a chunk");
        let mut rest = Vec::new();
        body.read_to_end(&mut rest).expect("the rest");
        assert_eq!((&first, rest.as_slice()), (b"From", &b": r1"[..]));
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_chunked_body_is_pulled_until_its_end_and_an_empty_chunk_is_not_it() {
        let mut chunks = vec![
            Some(b"C1".to_vec()),
            Some(Vec::new()),
            Some(b"P1".to_vec()),
            None,
        ];
        chunks.reverse();
        let asked = AtomicUsize::new(0);
        let mut body = chunked(|| {
            asked.fetch_add(1, Ordering::SeqCst);
            Ok(chunks.pop().flatten())
        });
        let mut read = Vec::new();
        body.read_to_end(&mut read).expect("read");
        assert_eq!(read, b"C1P1");
        assert_eq!(body.read(&mut [0; 4]).expect("the end"), 0);
        assert_eq!(
            asked.load(Ordering::SeqCst),
            4,
            "nothing asked past the end"
        );
    }

    #[test]
    fn a_chunked_body_reads_a_chunk_across_small_buffers_and_fails_as_its_source() {
        let mut given = false;
        let mut body = chunked(|| {
            if given {
                return Err(protocol_error("the share went away"));
            }
            given = true;
            Ok(Some(b"S1R1".to_vec()))
        });
        let mut two = [0u8; 2];
        body.read_exact(&mut two).expect("first half");
        assert_eq!(&two, b"S1");
        body.read_exact(&mut two).expect("second half");
        assert_eq!(&two, b"R1");
        let error = body.read(&mut two).expect_err("broken");
        assert!(error.to_string().contains("the share went away"), "{error}");
    }
}
