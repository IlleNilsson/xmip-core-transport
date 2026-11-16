//! What the Locations of one node share of a thing each of them names: the
//! one port a serial device has, which the operating system opens for one
//! handle at a time (ADR-0064, amendment 2026-11-15).
//!
//! The runtime makes one [`Shared`] for each node it starts and hands it to
//! every transport it builds there, once, as it builds it
//! ([`Configured::shared_by`](crate::Configured::shared_by)), as it hands
//! each the node's pins; a test hands in a [`Sharing`] of its own
//! (ADR-0068). A transport built outside a node is handed none, and keeps
//! its own for its own life.
//!
//! What is shared is a technology's own: it names the thing by what its
//! Locations name — a device's path — and the thing's type tells one
//! technology's from another's under the same name. Nothing here knows what
//! it holds, and nothing is shared between two nodes.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use xcore::lock::Locked;

/// A thing the Locations of a node share, as the node keeps it.
pub type Thing = Arc<dyn Any + Send + Sync>;

/// What the Locations of one node share.
pub trait Sharing: Send + Sync {
    /// The one thing of `kind` kept under `name` for every Location of this
    /// node that names it, `make` making it for the first that asks. `make`
    /// is called with the sharing held, so what it makes opens nothing until
    /// its first use.
    fn shared(&self, kind: TypeId, name: &str, make: &dyn Fn() -> Thing) -> Thing;
}

impl dyn Sharing {
    /// The one `T` the Locations of this node share under `name`, `make`
    /// making it for the first that asks. Where a sharing hands back another
    /// kind than it was asked for, the Location keeps one of its own, as a
    /// transport built outside a node does.
    #[must_use]
    pub fn one<T: Any + Send + Sync>(&self, name: &str, make: impl Fn() -> T) -> Arc<T> {
        self.shared(TypeId::of::<T>(), name, &|| Arc::new(make()) as Thing)
            .downcast::<T>()
            .unwrap_or_else(|_| Arc::new(make()))
    }
}

/// What the Locations of one node share, kept in memory for the node's
/// life: what the runtime hands every transport it builds on the node.
#[derive(Default)]
pub struct Shared {
    kept: Mutex<BTreeMap<TypeId, BTreeMap<String, Thing>>>,
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kept = self.kept.locked();
        let names: Vec<&str> = kept
            .values()
            .flat_map(BTreeMap::keys)
            .map(String::as_str)
            .collect();
        f.debug_struct("Shared").field("names", &names).finish()
    }
}

impl Sharing for Shared {
    fn shared(&self, kind: TypeId, name: &str, make: &dyn Fn() -> Thing) -> Thing {
        let mut kept = self.kept.locked();
        let named = kept.entry(kind).or_default();
        if let Some(thing) = named.get(name) {
            return Arc::clone(thing);
        }
        let thing = make();
        named.insert(name.to_string(), Arc::clone(&thing));
        thing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A device's port as a technology keeps it, by the name it is on.
    struct Port(&'static str);

    #[test]
    fn one_thing_is_made_for_every_location_naming_it() {
        let sharing: Arc<dyn Sharing> = Arc::new(Shared::default());
        let opened = AtomicUsize::new(0);
        let port = |name| {
            opened.fetch_add(1, Ordering::Relaxed);
            Port(name)
        };
        let first = sharing.one("COM3", || port("COM3"));
        let second = sharing.one("COM3", || port("COM3"));
        assert!(Arc::ptr_eq(&first, &second), "one port for both");
        let other = sharing.one("COM4", || port("COM4"));
        assert!(!Arc::ptr_eq(&first, &other), "another device, another port");
        assert_eq!((first.0, other.0), ("COM3", "COM4"));
        assert_eq!(opened.load(Ordering::Relaxed), 2);
        // Another technology's thing under the same name is its own.
        let counted = sharing.one("COM3", || 7_u32);
        assert_eq!(*counted, 7);
        assert_eq!(opened.load(Ordering::Relaxed), 2);
    }

    /// A sharing that hands back what nobody asked for.
    struct Wrong;

    impl Sharing for Wrong {
        fn shared(&self, _kind: TypeId, _name: &str, _make: &dyn Fn() -> Thing) -> Thing {
            Arc::new("not a port")
        }
    }

    #[test]
    fn a_location_handed_another_kind_keeps_one_of_its_own() {
        let sharing: Arc<dyn Sharing> = Arc::new(Wrong);
        let first = sharing.one("COM3", || Port("COM3"));
        let second = sharing.one("COM3", || Port("COM3"));
        assert_eq!(first.0, "COM3");
        assert!(!Arc::ptr_eq(&first, &second), "nothing shared");
    }
}
