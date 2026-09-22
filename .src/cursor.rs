//! Reading a binary message's fields in order.
//!
//! Kafka, TDS and `PostgreSQL`'s protocol each carried the same cursor over a
//! byte slice until 2026-09-22 — the same bounds check, the same error, the
//! same off-by-one waiting to happen in three places. What differs between
//! them is byte order and what a field means, so that stays with each
//! technology, which reads fixed-width fields through [`Cursor::array`] and
//! names them in its own protocol's words.

use crate::error::{Result, protocol_error};

/// Where reading has got to in a message.
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// True when nothing remains.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// What remains.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.at.min(self.bytes.len())..]
    }

    /// The next `count` bytes.
    ///
    /// # Errors
    /// Fewer than `count` bytes remain.
    pub fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| protocol_error("a field that runs past the message"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    /// Past the next `count` bytes.
    ///
    /// # Errors
    /// Fewer than `count` bytes remain.
    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.take(count).map(|_| ())
    }

    /// The next byte.
    ///
    /// # Errors
    /// Nothing remains.
    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// The next `N` bytes as an array, for a fixed-width field the caller
    /// reads in its protocol's byte order: `u32::from_le_bytes(c.array()?)`.
    ///
    /// # Errors
    /// Fewer than `N` bytes remain.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_in_order_and_what_remains_is_what_was_not_read() {
        let mut cursor = Cursor::new(&[1, 0, 2, 3, 4, 5]);
        assert_eq!(cursor.byte().expect("a byte"), 1);
        assert_eq!(u16::from_be_bytes(cursor.array().expect("two")), 2);
        cursor.skip(1).expect("one more");
        assert_eq!(cursor.remaining(), &[4, 5]);
        assert_eq!(cursor.take(2).expect("the rest"), &[4, 5]);
        assert!(cursor.is_empty());
    }

    #[test]
    fn a_field_past_the_end_is_refused_and_moves_nothing() {
        let mut cursor = Cursor::new(&[1, 2]);
        let error = cursor.take(3).expect_err("past the end");
        assert!(error.message.contains("runs past"), "{}", error.message);
        assert_eq!(cursor.remaining(), &[1, 2]);
        assert!(cursor.take(usize::MAX).is_err());
    }
}
