//! A SQL Location built from its settings: what every SQL Location
//! declares ([`super::super::declared`]), and what its technology's own
//! settings make of its wire ([`Wire::configured`]).

use xcore::settings::{Read, Settings};

use super::SqlTransport;
use crate::configured::Configured;
use crate::error::Result;
use crate::secured::Secured;
use crate::sql::declared::{QUERY, TIMEOUT, USER};
use crate::sql::wire::Wire;
use crate::sql::{Column, accept};

impl<W: Wire> Configured for SqlTransport<W> {
    /// The password the Location's secret holds, read at each login.
    fn secured_by(mut self, secret: Secured) -> Result<Self> {
        self.secret = Some(secret);
        Ok(self)
    }

    /// The address is the server's host and port: where a Location
    /// connects.
    const SETTINGS: &'static Settings = W::SETTINGS;

    /// The schemes a Location's address is entered with to choose this
    /// technology.
    const SCHEMES: &'static [&'static str] = W::SCHEMES;

    /// The password is the secret the Location names, never a setting.
    fn configured(address: &str, settings: &Read) -> Result<Self> {
        let wire = W::default().configured(settings)?;
        let mut transport = Self::new(
            address,
            settings.text(W::DATABASE),
            settings.text(USER.name),
        )
        .with_wire(wire)
        .holding(Column::configured(settings)?);
        if let Some(query) = settings.optional_text(QUERY.name) {
            transport = transport.with_query(query);
        }
        if let Some(statement) = settings.optional_text(accept::ACCEPT.name) {
            transport = transport.with_accept(statement);
        }
        if let Some(timeout) = settings.optional_duration(TIMEOUT.name) {
            transport = transport.timing_out_after(timeout);
        }
        Ok(transport)
    }
}
