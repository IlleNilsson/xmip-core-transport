//! What a SQL technology is to the one SQL transport ([`SqlTransport`]):
//! the dialect it writes in, its wire's client — logged in, a query's
//! rows, a statement run — and the far end its loopback serves. Everything
//! else a SQL Location does — its query's rows as Streams, the `accept`
//! statement after each row's cycle, the INSERT a send writes, the
//! connections kept per server and database, the settings it reads — is
//! [`SqlTransport`]'s, once. Until 2026-11-13 each of six technologies
//! carried all of it.

use std::net::TcpListener;
use std::time::Duration;

use ::sql::Dialect;
use xcore::settings::{Read, Settings};

use super::Rows;
use super::declared::DATABASE;
use super::sql_transport::SqlTransport;
use crate::error::Result;
use crate::login::Login;
use crate::pool::Pooled;
use crate::taken::Taken;

/// What a technology's loopback pair agrees on: the database the near end
/// logs in to, the user and password the far end demands and the near end
/// gives, and the `<table>/<column>` a payload is inserted into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    pub database: &'static str,
    pub user: &'static str,
    /// Empty where the login carries none.
    pub password: &'static str,
    pub target: &'static str,
}

/// A SQL technology: its dialect and its wire, the client a Location logs
/// in with and the far end its loopback serves. The value is what the
/// technology's own settings make of it — the TLS a login is guarded by —
/// and its default is a technology with none of them.
pub trait Wire: Clone + Default + Send + Sync + 'static {
    /// One logged-in connection, kept between statements while the server
    /// keeps it open.
    type Client: Send + 'static;

    /// The technology's name: what its transport answers to, and the
    /// scheme its origins open with.
    const NAME: &'static str;

    /// The schemes a Location's address and a send target are entered
    /// with: the name, and any its users also write (`sqlserver://`).
    const SCHEMES: &'static [&'static str];

    /// Every setting the technology declares, `technology` its module's
    /// name: the ones [`super::declared`] holds and its own.
    const SETTINGS: &'static Settings;

    /// The setting that names what a server keeps the rows in:
    /// [`DATABASE`], or Oracle's `service`.
    const DATABASE: &'static str = DATABASE.name;

    /// What the loopback pair agrees on.
    const PROBE: Probe;

    /// The SQL this technology writes and reads (`xmip-core-library-sql`).
    fn dialect() -> &'static Dialect;

    /// The dialect `client`'s session reads, where it may read another
    /// than [`Wire::dialect`]: `MariaDB`'s under `NO_BACKSLASH_ESCAPES`.
    /// `None` where it never does.
    fn session_dialect(client: &Self::Client) -> Option<&'static Dialect> {
        let _ = client;
        None
    }

    /// This technology as its own settings declare it — the TLS a login is
    /// guarded by — beyond what every SQL Location reads.
    ///
    /// # Errors
    /// A setting this build cannot honor.
    fn configured(self, settings: &Read) -> Result<Self> {
        let _ = settings;
        Ok(self)
    }

    /// Connect to `server` and log in to `database` as `login`, giving up
    /// on a server that stops for `timeout`.
    ///
    /// # Errors
    /// Where the server could not be reached or refused the login.
    fn connect(
        &self,
        server: &str,
        database: &str,
        login: &Login,
        timeout: Option<Duration>,
    ) -> Result<Self::Client>;

    /// Whether a kept client can carry another statement: no round trip.
    fn usable(client: &mut Self::Client) -> bool;

    /// Run `sql` on `client`; its rows, each column as text or NULL.
    ///
    /// # Errors
    /// Where the server went away or answered with an error.
    fn query(client: &mut Self::Client, sql: &str) -> Result<Rows<String>>;

    /// Run `sql` on `client` for its effect, committed where the server
    /// commits nothing until told.
    ///
    /// # Errors
    /// Where the server went away or answered with an error.
    fn execute(client: &mut Self::Client, sql: &str) -> Result<()>;

    /// What this technology's far end makes before a client can reach it,
    /// so no client's read waits on its making: the key a MySQL far end
    /// answers a login in the clear with. Nothing by default.
    fn ready_far_end(&self) {}

    /// The far end of `transport`'s loopback: one client accepted on
    /// `listener` and logged in as `transport` demands, and the Stream its
    /// first INSERT carries.
    ///
    /// # Errors
    /// Where nothing connected, the login failed, or the client closed
    /// without inserting.
    fn take_one(transport: &SqlTransport<Self>, listener: &TcpListener) -> Result<Taken>;
}

/// A logged-in connection, kept while the server keeps it open.
pub(crate) struct Kept<W: Wire>(pub(crate) W::Client);

impl<W: Wire> Pooled for Kept<W> {
    fn usable(&mut self) -> bool {
        W::usable(&mut self.0)
    }
}
