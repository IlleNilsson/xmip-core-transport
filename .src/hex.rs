//! Bytes as lower-case hex pairs, and the pairs back to bytes: the form a
//! DHCP option line, a TXT string, an SSDP header, a SQL binary literal
//! and a queue manager's identifier all spell bytes in.
//!
//! Ten technologies each wrote the two before 2026-09-14, and what two
//! technologies both need is shared through the capability (ADR-0044).

use std::fmt::Write;

use crate::error::{Result, protocol_error};

/// `bytes` as lower-case hex pairs, two digits a byte and nothing between.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The bytes `digits` spell, refused where they do not: an odd digit, or
/// one that is not hex.
///
/// # Errors
/// An odd number of digits, or a character that is not a hex digit.
pub fn unhex(digits: &str) -> Result<Vec<u8>> {
    if !digits.len().is_multiple_of(2) {
        return Err(protocol_error(format!(
            "an odd number of hex digits: {digits:?}"
        )));
    }
    (0..digits.len())
        .step_by(2)
        .map(|at| {
            digits
                .get(at..at + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| protocol_error(format!("not hex: {digits:?}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_back_and_refuses_what_is_not_hex() {
        assert_eq!(hex(&[0, 0x7f, 0xff]), "007fff");
        assert_eq!(hex(&[]), "");
        assert_eq!(unhex("007fff").expect("hex"), [0, 0x7f, 0xff]);
        assert_eq!(unhex("ABcd").expect("either case"), [0xab, 0xcd]);
        assert!(unhex("").expect("nothing").is_empty());
        assert!(unhex("abc").expect_err("odd").message.contains("odd"));
        assert!(
            unhex("zz")
                .expect_err("not hex")
                .message
                .contains("not hex")
        );
        let every: Vec<u8> = (0..=255).collect();
        assert_eq!(unhex(&hex(&every)).expect("round"), every);
    }
}
