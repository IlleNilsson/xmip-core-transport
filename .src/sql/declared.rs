//! The settings every SQL Location declares beside the payload column's
//! ([`super::COLUMN`], [`super::ENCODING`]) and the `accept` statement
//! ([`super::accept::ACCEPT`]): where it logs in, as whom, what a receive
//! runs, and how long a server is waited on. A technology lists these in
//! its declaration with its own, once each; until 2026-11-13 six
//! technologies wrote each of them out themselves.

#[cfg(feature = "tls")]
use xcore::settings::Read;
use xcore::settings::{Applies, Fixed, Kind, Presence, Setting};

/// What a Receive Location runs unless told otherwise.
pub const DEFAULT_QUERY: &str = "SELECT id, payload FROM inbox ORDER BY id";

/// The setting that names the database a Location logs in to.
pub const DATABASE: Setting = Setting {
    name: "database",
    kind: Kind::Text,
    presence: Presence::Required,
    meaning: "The database a Location logs in to and reads or inserts into, and a send \
              target's where it names none.",
    applies: Applies::Both,
};

/// The setting that names the user a Location logs in as.
pub const USER: Setting = Setting {
    name: "user",
    kind: Kind::Text,
    presence: Presence::Required,
    meaning: "The user a Location logs in as, its password the secret the Location names.",
    applies: Applies::Both,
};

/// The setting that holds the query a receive runs.
pub const QUERY: Setting = Setting {
    name: "query",
    kind: Kind::Text,
    presence: Presence::Default(Fixed::Text(DEFAULT_QUERY)),
    meaning: "The query a receive runs: the first column names the row, the last is the \
              Stream.",
    applies: Applies::Receive,
};

/// The setting that bounds the wait on a server.
pub const TIMEOUT: Setting = Setting {
    name: "timeout",
    kind: Kind::Duration,
    presence: Presence::Optional,
    meaning: "How long a server that stops mid-answer is waited on; unbounded when left out.",
    applies: Applies::Both,
};

/// The setting a SQL technology that guards its connections names its TLS
/// by: on or off, the meaning — when in the login TLS starts — the
/// technology's own.
pub const TLS: &str = "tls";

/// The setting that names what a server's certificate is checked for.
pub const TLS_NAME: Setting = Setting {
    name: "tls_name",
    kind: Kind::Text,
    presence: Presence::Optional,
    meaning: "The name the server's certificate is checked for; the address's host when left \
              out.",
    applies: Applies::Both,
};

/// What [`TLS`] and [`TLS_NAME`] ask of a client: the server's certificate
/// checked against the operating system's trust store, for the name given
/// or the address's host; `None` where [`TLS`] is off. One reading for
/// `MariaDB`, MySQL and DB2, which each carried it until 2026-11-13.
#[cfg(feature = "tls")]
#[must_use]
pub fn client_tls(settings: &Read) -> Option<tls::ClientSettings> {
    (settings.optional_boolean(TLS) == Some(true)).then(|| {
        let native = tls::ClientSettings::native();
        match settings.optional_text(TLS_NAME.name) {
            Some(name) => native.named(name),
            None => native,
        }
    })
}
