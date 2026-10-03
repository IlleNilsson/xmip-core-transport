//! A transport built from a Location: its address, and its settings read
//! through the declaration the technology makes of them.
//!
//! ADR-0064, amendment 2026-09-26: a technology declares its own settings in
//! its own crate, and that declaration is what the technology reads its
//! settings through, validates a Location's TOML at start and fills the
//! developer's form. `Configured` is the transport's half of it: the
//! declaration as [`Configured::SETTINGS`], and the one constructor that
//! takes what the declaration read. The shape is `xcore::settings`, shared
//! with every other capability's technologies.
//!
//! A companion to [`Transport`](crate::Transport) rather than a part of it:
//! `Transport` mirrors the C ABI's vtable (ADR-0010), and a declaration is
//! plain data a surface reads without building anything.

use xcore::settings::{Applies, Given, Read, Settings};

use crate::error::Result;
use crate::node::NodeLocation;
use crate::protocol::Transport;

/// A transport technology a Location can be configured with.
pub trait Configured: Transport + Sized {
    /// Every setting this technology takes beyond the Location's address —
    /// an empty list when it takes none — with `technology` its module name,
    /// `env!("CARGO_PKG_NAME")`.
    const SETTINGS: &'static Settings;

    /// This technology at `address`, in its own terms, with the settings its
    /// declaration read.
    ///
    /// # Errors
    /// When the address or a setting is one the technology cannot use —
    /// never for a setting the declaration already refuses.
    fn configured(address: &str, settings: &Read) -> Result<Self>;

    /// Read what a Location on `side` gave through [`Configured::SETTINGS`],
    /// and build this technology from it: the one way from a Location's
    /// table to a transport.
    ///
    /// # Errors
    /// Every setting the declaration refuses, each naming the technology and
    /// the setting; then whatever [`Configured::configured`] refuses.
    fn open(address: &str, side: Applies, given: &[(String, Given)]) -> Result<Self> {
        let read = Self::SETTINGS.read(side, given)?;
        Self::configured(address, &read)
    }

    /// This transport on the node at `node`: what the runtime calls once,
    /// as it builds a Location's transport, right after
    /// [`Configured::open`] (the owner's *Option A*, 2026-10-03: the runtime
    /// gives every transport its node's identity once, as it builds it,
    /// `node.rs`). A technology
    /// that records at the far end who holds what — a claimed file's name
    /// (ADR-0024, amendment 2026-09-26) — keeps it, and returns there what
    /// the node held before it last stopped. Nothing is kept by default.
    ///
    /// # Errors
    /// Where the technology could not return what the node held before:
    /// the far end could not be read.
    fn on_node(self, _node: &NodeLocation) -> Result<Self> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrived::Arrived;
    use crate::direction::Directions;
    use xcore::settings::{Fixed, Kind, Presence, Setting};

    struct Example {
        topic: String,
        wait_ms: u128,
    }

    impl Transport for Example {
        fn name(&self) -> &'static str {
            "example"
        }
        fn directions(&self) -> Directions {
            Directions::BOTH
        }
        fn receive(&self) -> Result<Vec<Arrived>> {
            Ok(Vec::new())
        }
        fn arrivals(&self) -> crate::Arrivals {
            crate::Arrivals::Unordered("nothing ever arrives")
        }
        fn send(&self, _target: &str, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }
    }

    impl Configured for Example {
        const SETTINGS: &'static Settings = &Settings {
            technology: "xmip-core-transport-example",
            settings: &[
                Setting {
                    name: "topic",
                    kind: Kind::Text,
                    presence: Presence::Required,
                    meaning: "The topic a Location reads or writes.",
                    applies: Applies::Both,
                },
                Setting {
                    name: "wait",
                    kind: Kind::Duration,
                    presence: Presence::Default(Fixed::Text("5s")),
                    meaning: "How long one receive waits.",
                    applies: Applies::Receive,
                },
            ],
        };

        fn configured(_address: &str, settings: &Read) -> Result<Self> {
            Ok(Self {
                topic: settings.text("topic").to_string(),
                wait_ms: settings
                    .optional_duration("wait")
                    .map_or(0, |wait| wait.as_millis()),
            })
        }
    }

    #[test]
    fn a_location_is_read_through_the_declaration_and_built() {
        let given = [("topic".to_string(), Given::Text("orders".to_string()))];
        let built = Example::open("host:1", Applies::Receive, &given).expect("built");
        assert_eq!(built.topic, "orders");
        assert_eq!(built.wait_ms, 5000);

        let sent = Example::open("host:1", Applies::Send, &given).expect("built");
        assert_eq!(sent.wait_ms, 0, "a Send Location reads no receive setting");
    }

    #[test]
    fn a_refused_setting_is_a_permanent_failure_naming_it() {
        let Err(refused) = Example::open("host:1", Applies::Receive, &[]) else {
            panic!("topic is required");
        };
        assert!(!refused.retryable);
        assert!(refused.message.contains("\"topic\""), "{}", refused.message);
        assert!(refused.message.contains("xmip-core-transport-example"));
    }
}
