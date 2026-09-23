//! X.690 basic encoding rules, the tag-length-value every ASN.1 protocol
//! frames with. MMS, GOOSE and SNMP say what a tag means in their own
//! crates; the reader and writer they share lived here from 2026-09-14
//! (ADR-0044) and in `xmip-core-library-asn1` since 2026-09-22, where the Kerberos,
//! LDAP and X.509 technologies read the same bytes. Named here as `ber` so a
//! transport technology reaches it through its parent, and a failure turns
//! into a protocol error with `?`.

pub use asn1::{
    Asn1Error, BIT_STRING, BOOLEAN, INTEGER, NULL, OBJECT_IDENTIFIER, OCTET_STRING, SEQUENCE,
    VISIBLE_STRING, context, find, integer, read, read_all, read_integer, tlv, write_tlv,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;

    #[test]
    fn a_malformed_element_is_a_permanent_protocol_error() {
        let read_one = |bytes: &[u8]| -> Result<u8> { Ok(read(bytes)?.0) };
        let error = read_one(&[0x04, 0x05, 1]).expect_err("past the end");
        assert!(!error.retryable);
        assert!(error.message.contains("says 5 bytes"), "{}", error.message);
        assert_eq!(read_one(&tlv(NULL, &[])).expect("null"), NULL);
    }
}
