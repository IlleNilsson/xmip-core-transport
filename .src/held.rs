//! A far end held in this process: a device on a simulated bus, a server
//! in a loopback radio, a directory, a pipe made and waiting (ADR-0051).
//!
//! Twenty technologies each declared this struct — a `Holding`, a
//! `Served`, an `OnTheBus`, a `Directory` — with an address and one call to
//! make when the round takes, and its `FarEnd`, until 2026-09-24. The
//! address and the take are the technology's; the far end that carries them
//! is this one.

use crate::arrived::Arrived;
use crate::error::Result;
use crate::loopback::FarEnd;

/// A far end at `address` whose one exchange is `take`.
pub struct Held<F> {
    address: String,
    take: F,
}

impl<F> Held<F>
where
    F: FnOnce() -> Result<Arrived> + Send,
{
    /// A far end the near end reaches at `address`, and what taking its one
    /// exchange is.
    #[must_use]
    pub fn new(address: impl Into<String>, take: F) -> Self {
        Self {
            address: address.into(),
            take,
        }
    }
}

impl<F> FarEnd for Held<F>
where
    F: FnOnce() -> Result<Arrived> + Send,
{
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        (self.take)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_far_end_reports_its_address_and_takes_once() {
        let far = Box::new(Held::new("bus://loopback/1", || {
            Ok(Arrived::new("bus://loopback/1", b"held".to_vec()))
        }));
        assert_eq!(far.address(), "bus://loopback/1");
        assert_eq!(far.take_one().expect("take").bytes, b"held");
    }
}
