//! Checksums more than one technology frames with.
//!
//! IEEE 802.15.4 closes every frame with the same frame check sequence, and
//! every technology on that radio — Thread and `WirelessHART` — carried its own
//! copy of it until 2026-09-22. A checksum only one technology uses stays in
//! that technology (DNP3's, EN 13757-4's).

/// CRC-16/KERMIT: the ITU-T polynomial, reflected, seeded with zero — the
/// IEEE 802.15.4 frame check sequence, sent least significant byte first.
#[must_use]
pub fn kermit(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |mut crc, byte| {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
        crc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kermit_gives_the_catalogued_check_value() {
        assert_eq!(kermit(b"123456789"), 0x2189);
        assert_eq!(kermit(b""), 0);
    }
}
