# xmip-core-transport

Direction-neutral Transport contracts for moving immutable Streams between Xmip and endpoints.

Receive and Send own orchestration, ports, groups, and locations. Transport owns byte movement and resource claims; it does not own authentication, message representation, contract evaluation, or operation semantics.

This crate is the common boundary: the `Transport` trait, `Arrived`, `Directions`, the error vocabulary, resource claims, the line-oriented wire helpers, and what several technologies frame with: X.690, a binary cursor, the 802.15.4 checksum. Nothing in it names a protocol. Each transport technology is its own repository (ADR-0010) mounted directly under this one and depends on this crate, never the reverse; `architecture.toml` names them and carries each one's maturity, and this file does not repeat the list.
