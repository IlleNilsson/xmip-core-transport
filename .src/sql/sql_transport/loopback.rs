//! A SQL technology as both ends of one exchange (ADR-0051): the far end
//! is its [`Wire::take_one`], on a listener bound at the transport's
//! server; the near end a send from a fresh transport at the bound
//! address, into the table and column its [`Wire::PROBE`] names.

use std::net::TcpListener;

use super::SqlTransport;
use crate::arrival_identity::ArrivalIdentity;
use crate::error::Result;
use crate::listening::{Accepting, Listening};
use crate::loopback::{FarEnd, Loopback};
use crate::protocol::Transport;
use crate::sql::wire::Wire;
use crate::taken::Taken;

impl<W: Wire> Accepting for SqlTransport<W> {
    fn take_one(self, listener: &TcpListener) -> Result<Taken> {
        W::take_one(&self, listener)
    }
}

impl<W: Wire> Loopback for SqlTransport<W> {
    fn arrival_identity(&self) -> ArrivalIdentity {
        ArrivalIdentity::Unnamed(
            "a row names no sender: the database it was read from is in its origin",
        )
    }

    /// Ready before it is bound ([`Wire::ready_far_end`]): a client
    /// connects as soon as the address is handed out.
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        self.wire.ready_far_end();
        Ok(Box::new(Listening::new(self.clone(), self.bind()?)))
    }

    /// INSERT the payload as one column of one row, as the column is
    /// declared to hold it, from a fresh near end logging in to `address`
    /// as this transport does.
    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        let near = Self {
            server: address.to_string(),
            ..self.clone()
        };
        near.send(W::PROBE.target, payload)
    }
}
