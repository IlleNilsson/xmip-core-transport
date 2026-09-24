//! A payload against the most a protocol carries whole.
//!
//! A ceiling is a fact about the protocol, written where it comes from
//! (ADR-0028, amendment 2026-09-09; ADR-0051 clause 2): the number is the
//! technology's — one datagram's 65 507 bytes, one SQS message's 256 KiB —
//! and so is the phrase that says what carries it. The refusal is one
//! sentence everywhere, and twenty-eight sends, encoders and loopbacks and
//! the Playground each wrote it until 2026-09-24.

use crate::error::{Result, protocol_error};

/// `Ok` when `size` bytes fit under `ceiling`; otherwise the refusal,
/// `"<size> bytes is over the <ceiling> <carrier>"`, where `carrier` says
/// what holds that many: `"one datagram carries"`, `"one ISDU carries"`.
///
/// # Errors
/// Where `size` is over `ceiling`: a failure that will say the same thing
/// next time.
pub fn within(size: usize, ceiling: usize, carrier: &str) -> Result<()> {
    if size > ceiling {
        return Err(protocol_error(format!(
            "{size} bytes is over the {ceiling} {carrier}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_at_the_ceiling_fits_and_one_byte_over_is_refused_permanently() {
        assert!(within(8, 8, "one test frame carries").is_ok());
        let error = within(9, 8, "one test frame carries").expect_err("over");
        assert_eq!(
            error.message,
            "9 bytes is over the 8 one test frame carries"
        );
        assert!(!error.retryable);
    }
}
