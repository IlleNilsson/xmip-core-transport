//! The payloads a loopback round is exercised with: the shapes a framing
//! fault changes, and the sizes a datagram, a window or a length prefix
//! trips over.
//!
//! Sixty-seven technologies each carried this list in their tests before
//! 2026-09-14, forty-one of them byte for byte; a fixture two technologies
//! both need is shared through the capability like anything else
//! (ADR-0044). Compiled for this crate's own tests and for a technology
//! that asks for the `test-support` feature from its dev-dependencies, so
//! a production build never carries a fixture.

/// The shapes a transport is most likely to change: nothing, one byte,
/// every byte value, a run of NULs, high bytes, and line endings alone.
#[must_use]
pub fn edge_payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("empty", Vec::new()),
        ("one byte", vec![0x2a]),
        ("every byte", (0..=255).collect()),
        ("nul run", vec![0; 512]),
        ("high bytes", vec![0xff; 512]),
        ("crlf storm", b"\r\n".repeat(400)),
    ]
}

/// The sizes a transport is most likely to cut: either side of what one
/// Ethernet frame carries of UDP, the largest datagram, one over sixteen
/// bits, and a mebibyte. Each is [`patterned`], so a truncation, a reorder
/// or a duplicate shows.
#[must_use]
pub fn sized_payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("mtu minus one", patterned(1_471)),
        ("mtu", patterned(1_472)),
        ("mtu plus one", patterned(1_473)),
        ("udp maximum", patterned(65_507)),
        ("sixteen bits plus one", patterned(65_537)),
        ("a mebibyte", patterned(1 << 20)),
    ]
}

/// `len` bytes a truncation, a reorder or a duplicate would change.
#[must_use]
pub fn patterned(len: usize) -> Vec<u8> {
    (0..len)
        .map(|at| u8::try_from((at * 31 + at / 251) % 256).unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edges_are_the_six_shapes_and_the_sizes_are_patterned() {
        let edges = edge_payloads();
        assert_eq!(edges.len(), 6);
        assert!(edges[0].1.is_empty());
        assert_eq!(edges[2].1.len(), 256);
        assert_eq!(edges[5].1, b"\r\n".repeat(400));
        let sizes = sized_payloads();
        assert_eq!(sizes.len(), 6);
        assert_eq!(sizes[3].1.len(), 65_507);
        assert_eq!(sizes[5].1.len(), 1 << 20);
        let pattern = patterned(1_000);
        assert_ne!(pattern[..500], pattern[500..], "position shows");
        assert_eq!(patterned(0), Vec::<u8>::new());
    }
}
