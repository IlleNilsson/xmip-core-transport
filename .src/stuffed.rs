//! The dot-stuffed block mail speaks: SMTP's DATA (RFC 5321 section 4.5.2)
//! and POP3's multi-line response (RFC 1939 section 3) are the same shape —
//! bytes, a period doubled where one starts a line, ended by a line that is
//! one period.
//!
//! Until 2026-09-10 both technologies cut the bytes into lines, wrote each
//! with CRLF and joined them back with CRLF: a bare LF came back as CRLF and
//! a trailing CRLF was lost, which the playground's edge payloads found. This
//! writes the bytes as they are — a period doubled at the start and after
//! every CRLF, the terminator after — and reads them back to the first
//! `CRLF . CRLF`, so what was sent is what arrives. Text mail is on the wire
//! exactly as before; only bytes that were never lines survive now too. A
//! peer that reads by lines and canonicalises a bare LF is doing what the
//! RFCs allow; between two Xmip ends nothing is canonicalised.

use std::io::BufRead;

use crate::error::{Result, classify, protocol_error};

/// The sequence that ends a block: the writer's own CRLF, a period, CRLF.
const TERMINATOR: &[u8] = b"\r\n.\r\n";

/// `bytes` as the block goes on the wire: a period doubled at the start and
/// after every CRLF, then the terminator.
#[must_use]
pub fn stuff(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 8);
    let mut at_line_start = true;
    for &byte in bytes {
        if at_line_start && byte == b'.' {
            out.push(b'.');
        }
        out.push(byte);
        at_line_start = byte == b'\n' && out.len() >= 2 && out[out.len() - 2] == b'\r';
    }
    out.extend_from_slice(TERMINATOR);
    out
}

/// The bytes a block on the wire carried: everything before the first
/// terminator, the doubled periods undone.
///
/// # Errors
/// The connection closed before the terminator, or the block passed `max`.
pub fn read_stuffed(reader: &mut impl BufRead, max: usize) -> Result<Vec<u8>> {
    let mut wire = Vec::new();
    loop {
        let read = reader
            .read_until(b'\n', &mut wire)
            .map_err(|e| classify("reading a dot-stuffed block", &e))?;
        if read == 0 {
            return Err(protocol_error("a block that ended before its terminator"));
        }
        if wire.len() > max + TERMINATOR.len() {
            return Err(protocol_error("a block over the size Xmip will read"));
        }
        if wire == b".\r\n" {
            return Ok(Vec::new());
        }
        if wire.ends_with(TERMINATOR) {
            let body = &wire[..wire.len() - TERMINATOR.len()];
            return Ok(unstuff(body));
        }
    }
}

/// A doubled period at the start of a line back to one.
fn unstuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut at_line_start = true;
    let mut skip = false;
    for (index, &byte) in body.iter().enumerate() {
        if skip {
            skip = false;
        } else if at_line_start && byte == b'.' && body.get(index + 1) == Some(&b'.') {
            skip = true;
            continue;
        }
        out.push(byte);
        at_line_start = byte == b'\n' && index >= 1 && body[index - 1] == b'\r';
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(bytes: &[u8]) -> Vec<u8> {
        read_stuffed(&mut stuff(bytes).as_slice(), 1 << 20).expect("read")
    }

    #[test]
    fn text_goes_on_the_wire_as_it_always_did() {
        assert_eq!(stuff(b"one\r\ntwo"), b"one\r\ntwo\r\n.\r\n");
        assert_eq!(stuff(b".hidden\r\n..x"), b"..hidden\r\n...x\r\n.\r\n");
        assert_eq!(stuff(b""), b"\r\n.\r\n");
    }

    #[test]
    fn every_shape_of_bytes_comes_back_whole() {
        for bytes in [
            &b""[..],
            b".",
            b"a",
            b"a\r\n",
            b"a\r\n\r\n",
            b"a\nb",
            b"\r\n.\r\n",
            b".\r\n.\r\n.",
            b"\n.",
            b"\r",
            b"\x00\xff\r\n\xfe",
        ] {
            assert_eq!(round(bytes), bytes, "{bytes:?}");
        }
        let every: Vec<u8> = (0..=255).collect();
        assert_eq!(round(&every), every);
        let storm = b"\r\n".repeat(400);
        assert_eq!(round(&storm), storm);
    }

    #[test]
    fn a_standard_sender_reads_and_a_cut_block_refuses() {
        assert_eq!(
            read_stuffed(&mut &b"one\r\ntwo\r\n.\r\n"[..], 1 << 20).expect("read"),
            b"one\r\ntwo"
        );
        assert_eq!(
            read_stuffed(&mut &b".\r\n"[..], 1 << 20).expect("read"),
            b""
        );
        assert!(read_stuffed(&mut &b"never ends\r\n"[..], 1 << 20).is_err());
        assert!(read_stuffed(&mut &b"too long\r\n.\r\n"[..], 3).is_err());
    }
}
