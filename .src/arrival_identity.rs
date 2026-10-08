//! What an arrival says about who sent it, beyond where it came from: the
//! names a technology writes on every arrival it hands over, and the one
//! check every loopback round makes that they arrived.
//!
//! The owner, 2026-10-06: *The first stream will come from somewhere, it will
//! carry its identity via transport or on its message* — and *that goes for
//! all streams*. Identity travels on the transport where the technology makes
//! it feasible (ADR-0019 clause 5); what a transport observed is written on
//! the arrival under the names `xmip-core-context` declares
//! (`context::property`, ADR-0019 amendment 2026-09-24), and the runtime hands
//! it to the identity gates. A technology says here which names it writes,
//! or why it has nothing beyond the origin to say; [`crate::Loopback::round`]
//! holds every technology to what it says.

use std::net::SocketAddr;

use context::property::{FILE_GROUP, FILE_MODE, FILE_OWNER, PEER_ADDRESS, PEER_MAC};

use crate::error::{Result, protocol_error};
use crate::taken::Taken;

/// The names a technology's arrival carries its sender under, or why it
/// carries nothing beyond its origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrivalIdentity {
    /// Written on every arrival, each a `context::property` name:
    /// [`PEER_ADDRESS`] for a socket's peer, a header's name, the SSH key.
    Named(&'static [&'static str]),
    /// Nothing beyond the origin, in the technology's words: a file's path
    /// is its origin, a bus frame names no sender.
    Unnamed(&'static str),
}

impl ArrivalIdentity {
    /// The socket peer, and nothing else: what a connection or a datagram
    /// carries of its sender.
    pub const PEER: Self = Self::Named(&[PEER_ADDRESS]);

    /// The peer's hardware address, and nothing else: what a link-layer
    /// frame carries of its sender.
    pub const PEER_MAC: Self = Self::Named(&[PEER_MAC]);

    /// The socket peer and the hardware address the message names: what a
    /// datagram from a link-layer client carries of it.
    pub const PEER_AND_MAC: Self = Self::Named(&[PEER_ADDRESS, PEER_MAC]);

    /// Whether `taken` carries every name this says it does.
    ///
    /// # Errors
    /// The names it says and the arrival dropped.
    pub fn check(&self, taken: &Taken) -> Result<()> {
        let Self::Named(names) = self else {
            return Ok(());
        };
        let dropped: Vec<&str> = names
            .iter()
            .copied()
            .filter(|name| taken.observation(name).is_none())
            .collect();
        if dropped.is_empty() {
            Ok(())
        } else {
            Err(protocol_error(format!(
                "the arrival from {} dropped who sent it: {}",
                taken.origin_uri,
                dropped.join(", ")
            )))
        }
    }
}

/// The socket peer as an arrival carries it: [`PEER_ADDRESS`], the address
/// and port as the socket reported them — `192.0.2.10:4711`,
/// `[2001:db8::1]:443`.
#[must_use]
pub fn peer(peer: SocketAddr) -> (String, String) {
    (PEER_ADDRESS.to_string(), peer.to_string())
}

/// The peer's hardware address as an arrival carries it: [`PEER_MAC`], as
/// the link reported it — `02:00:00:00:00:01`.
#[must_use]
pub fn peer_mac(mac: &impl ToString) -> (String, String) {
    (PEER_MAC.to_string(), mac.to_string())
}

/// Who owns a file and what it permits, as an arrival carries them: the
/// owning user and group ids and the permission bits, octal — `0640` —
/// whether a local folder's metadata said so or a share's attributes did.
#[must_use]
pub fn file_owner(owner: u32, group: u32, mode: u32) -> Vec<(String, String)> {
    vec![
        (FILE_OWNER.to_string(), owner.to_string()),
        (FILE_GROUP.to_string(), group.to_string()),
        (FILE_MODE.to_string(), format!("{:04o}", mode & 0o7777)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_identity_is_held_to_its_names_and_an_unnamed_one_to_nothing() {
        let peer: SocketAddr = "192.0.2.10:4711".parse().expect("an address");
        let carried = Taken::new("tcp://192.0.2.10:4711", b"x".to_vec()).from_peer(peer);
        let bare = Taken::new("tcp://192.0.2.10:4711", b"x".to_vec());

        assert_eq!(carried.observation(PEER_ADDRESS), Some("192.0.2.10:4711"));
        assert!(ArrivalIdentity::PEER.check(&carried).is_ok());
        let dropped = ArrivalIdentity::PEER.check(&bare).expect_err("dropped");
        assert!(dropped.message.contains(PEER_ADDRESS), "{dropped}");
        assert!(ArrivalIdentity::Unnamed("a path").check(&bare).is_ok());
    }
}
