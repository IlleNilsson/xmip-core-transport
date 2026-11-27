//! An IEEE 802.15.4 radio: where the frames of a technology above the MAC go
//! and come from — Thread's 6LoWPAN, Zigbee's network layer, the DLPDU of
//! `WirelessHART` — each to a short address on the radio's PAN, the MAC
//! header and its check sequence the radio's own.
//!
//! Two kinds of radio are one: the kernel's, an interface such as `wpan0`
//! that Linux's MAC drives (`net::ieee802154::Interface`, opened by
//! [`interface`]), and each technology's own in process, the node at its
//! far end, which every test and every box without a radio drives. Thread
//! and Zigbee each declared this trait for themselves, identically, and
//! `WirelessHART` one of its own, each frame carrying a MAC header the
//! technology wrote, until 2026-11-27; it lives here, where each already
//! depends, as `line::Line` does for a serial line (ADR-0051, amendment
//! 2026-11-27).

use std::sync::Arc;
use std::time::Duration;

use net::ieee802154::{Address, Heard, Interface};

use crate::error::{Result, protocol_error};
use crate::waiting::Waiting;

/// Where a technology's frames go and come from: the air, as one node
/// hears it.
pub trait Radio: Send + Sync {
    /// The radio's name, for the origin URI: an interface's, or `loopback`.
    fn name(&self) -> &str;

    /// Hand `payload`, one frame's, to the MAC for the short address
    /// `destination` on the radio's PAN.
    ///
    /// # Errors
    /// Where the radio refused it: over what one frame carries, or the
    /// kernel's refusal.
    fn transmit(&self, destination: u16, payload: &[u8]) -> Result<()>;

    /// The next frame sent to this node and the short address it came from,
    /// or `None` when nothing arrived within `timeout`.
    ///
    /// # Errors
    /// Where the radio could not be read.
    fn receive(&self, timeout: Duration) -> Result<Option<Heard>>;

    /// How a receive meets a quiet air: an interface waits for the next
    /// frame, a node in process looks and returns at once.
    fn waiting(&self) -> Waiting;
}

/// The kernel's MAC: a frame to a short address on the interface's PAN,
/// from the interface's own.
impl Radio for Interface {
    fn name(&self) -> &str {
        Self::name(self)
    }

    fn transmit(&self, destination: u16, payload: &[u8]) -> Result<()> {
        Ok(self.send(destination, payload)?)
    }

    fn receive(&self, timeout: Duration) -> Result<Option<Heard>> {
        Ok(Self::receive(self, timeout)?)
    }

    fn waiting(&self) -> Waiting {
        Waiting::Waits("a read of an 802.15.4 interface waits for its next frame")
    }
}

/// The kernel's interface `name` as a radio, and where it is on the air.
///
/// # Errors
/// As `net::ieee802154::Interface::open`: not Linux, no 802.15.4 sockets,
/// no such interface, or one with no address yet, each in words.
pub fn interface(name: &str) -> Result<(Arc<dyn Radio>, Address)> {
    let opened = Interface::open(name)?;
    let address = opened.address();
    Ok((Arc::new(opened), address))
}

/// `Ok` where a Location's `setting`, which says `said`, is what the
/// interface `name` is on the air, `is`: the kernel's MAC puts the
/// interface's own address and PAN on every frame, so a setting naming
/// another would be a node the air never hears.
///
/// # Errors
/// Where the two differ, naming both.
pub fn agreeing(name: &str, setting: &str, said: u16, is: u16) -> Result<()> {
    if said == is {
        return Ok(());
    }
    Err(protocol_error(format!(
        "{name} is {is:#06x} on the air, and `{setting}` says {said:#06x}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_held_to_the_interface() {
        assert!(agreeing("wpan0", "rloc16", 1, 1).is_ok());
        let refused = agreeing("wpan0", "rloc16", 7, 1).expect_err("another address");
        assert_eq!(
            refused.message,
            "wpan0 is 0x0001 on the air, and `rloc16` says 0x0007"
        );
        assert!(!refused.retryable, "the configuration's to change");
    }

    #[test]
    fn an_interface_this_machine_has_not_is_refused_in_words() {
        let Err(refused) = interface("xmip-absent") else {
            panic!("no such interface");
        };
        assert!(refused.message.contains("xmip-absent"), "{refused}");
    }
}
