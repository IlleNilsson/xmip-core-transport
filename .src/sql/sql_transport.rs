//! The one SQL transport: Streams that arrive as rows and leave as rows,
//! over any technology's [`Wire`].
//!
//! One row is one Stream. A Receive Location runs its query —
//! [`DEFAULT_QUERY`] unless told otherwise — on the connection kept for its
//! server and database, and hands each row up whole: the last column is
//! what Xmip carries, the first is where it came from, its origin
//! `<scheme>://server/database?row=<first column>`. The query only reads;
//! the `accept` statement consumes a row after its cycle ([`accept`]). A
//! Send Location inserts the Stream as one column of one row, where its
//! target says ([`destination`]), on the connection kept for that server
//! and database. What the column holds is the Location's to declare
//! ([`Column`]). Rows are artifacts and no receive holds a transaction to
//! claim one in (ADR-0024).
//!
//! What differs between technologies is their [`Wire`]: the dialect, the
//! client, the far end. Until 2026-11-13 each of six technologies carried
//! this file's code itself, `MariaDB`'s and MySQL's differing in 21 of 266
//! lines.

mod configured;
mod loopback;

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use super::declared::DEFAULT_QUERY;
use super::wire::{Kept, Wire};
use super::{Column, accept, destination};
use crate::acknowledgement::Acknowledgement;
use crate::arrivals::Arrivals;
use crate::arrived::Arrived;
use crate::claim::{NoNativeClaim, ResourceClaim};
use crate::direction::Directions;
use crate::error::Result;
use crate::login::Login;
use crate::loopback::LOOPBACK_TIMEOUT;
use crate::outgoing::Outgoing;
use crate::pool::Pool;
use crate::protocol::Transport;
use crate::secured::{self, Secured};
use crate::sending::Sending;
use crate::socket;
use crate::taken::Taken;
use crate::waiting::Waiting;

/// A Location over a SQL database, its technology `W`.
#[derive(Clone)]
pub struct SqlTransport<W: Wire> {
    /// The secret its Location names, whose password the login presents,
    /// read at each login (ADR-0019, amendment 2026-10-29).
    secret: Option<Secured>,
    server: String,
    database: String,
    login: Login,
    query: String,
    /// The statement run on a row's verdict, its name bound in.
    accept: Option<String>,
    /// The rows refused and left, not received again while unchanged.
    /// Cloned, the same memory.
    refused: accept::RefusedRows,
    column: Column,
    timeout: Option<Duration>,
    wire: W,
    /// The connections a send inserts on and a receive queries on, logged
    /// in once per server and database and kept. Cloned, the same pool.
    connections: Pool<Kept<W>>,
}

impl<W: Wire> SqlTransport<W> {
    /// Speak to the server at `server`, on `database`, as `user` with no
    /// password until one is given, over the technology's default wire.
    #[must_use]
    pub fn new(
        server: impl Into<String>,
        database: impl Into<String>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            secret: None,
            server: server.into(),
            database: database.into(),
            login: Login::new(user, ""),
            query: DEFAULT_QUERY.to_string(),
            accept: None,
            refused: accept::RefusedRows::default(),
            column: Column::Binary,
            timeout: None,
            wire: W::default(),
            connections: Pool::new(),
        }
    }

    /// Both ends on this machine: an ephemeral local port, the login the
    /// technology's [`Wire::PROBE`] agrees on, the loopback timeout.
    #[must_use]
    pub fn loopback() -> Self {
        Self::new("127.0.0.1:0", W::PROBE.database, W::PROBE.user)
            .with_password(W::PROBE.password)
            .timing_out_after(LOOPBACK_TIMEOUT)
    }

    /// The password the login presents; none where empty.
    #[must_use]
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.login.password = password.into();
        self
    }

    /// The query a receive runs: the first column is the row's name, the
    /// last is the Stream.
    #[must_use]
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    /// The statement run once a row's cycle accepted it, the row's name in
    /// place of the dialect's first parameter ([`accept`]).
    #[must_use]
    pub fn with_accept(mut self, statement: impl Into<String>) -> Self {
        self.accept = Some(statement.into());
        self
    }

    /// What the payload column holds: bytes unless declared otherwise.
    #[must_use]
    pub fn holding(mut self, column: Column) -> Self {
        self.column = column;
        self
    }

    /// Give up on a server that stops mid-answer.
    #[must_use]
    pub fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The technology as `wire` is: the TLS its login is guarded by.
    #[must_use]
    pub fn with_wire(mut self, wire: W) -> Self {
        self.wire = wire;
        self
    }

    /// The server a Location connects to.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// The database a Location logs in to.
    #[must_use]
    pub fn database(&self) -> &str {
        &self.database
    }

    /// The user and password a login presents, and a far end demands.
    #[must_use]
    pub const fn login(&self) -> &Login {
        &self.login
    }

    /// The query a receive runs.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The statement a row's acceptance runs, where one is declared.
    #[must_use]
    pub fn accept(&self) -> Option<&str> {
        self.accept.as_deref()
    }

    /// What the payload column holds.
    #[must_use]
    pub const fn column(&self) -> Column {
        self.column
    }

    /// How long a server is waited on; unbounded where `None`.
    #[must_use]
    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// The technology as its settings made it.
    #[must_use]
    pub const fn wire(&self) -> &W {
        &self.wire
    }

    /// How many connections this transport has logged in: one per server
    /// and database while the server keeps them, however many statements.
    #[must_use]
    pub fn opened(&self) -> usize {
        self.connections.opened()
    }

    /// Log in to the server.
    ///
    /// # Errors
    /// Where the server could not be reached or refused the login.
    pub fn connect(&self) -> Result<W::Client> {
        self.connect_to(&self.server, &self.database)
    }

    /// Bind as the far end clients connect to, and report the address.
    ///
    /// # Errors
    /// Where the address is taken, malformed, or not permitted.
    pub fn bind(&self) -> Result<(TcpListener, String)> {
        socket::bind_tcp(&self.server)
    }

    /// The Stream a far end took: `bytes` a client at `peer` inserted into
    /// `table` and `column` of `database`, its origin
    /// `<scheme>://peer/database/table/column`.
    #[must_use]
    pub fn inserted(
        peer: SocketAddr,
        database: &str,
        table: &str,
        column: &str,
        bytes: Vec<u8>,
    ) -> Taken {
        let origin = format!("{}://{peer}/{database}/{table}/{column}", W::NAME);
        Taken::new(origin, bytes)
    }

    fn connect_to(&self, server: &str, database: &str) -> Result<W::Client> {
        let login = secured::logged_in(&self.login, self.secret.as_ref(), W::NAME)?;
        self.wire.connect(server, database, &login, self.timeout)
    }

    /// One query or statement on the connection kept for `server` and
    /// `database`, logged in on the first.
    fn exchange<T>(
        &self,
        server: &str,
        database: &str,
        mut work: impl FnMut(&mut W::Client) -> Result<T>,
    ) -> Result<T> {
        self.connections.exchange(
            &format!("{server}/{database}"),
            || self.connect_to(server, database).map(Kept),
            |kept| work(&mut kept.0),
        )
    }

    /// What a row named `name` is told on its verdict: the `accept`
    /// statement, on the connection kept for the server and database.
    fn acknowledgement(
        shared: &Arc<Self>,
        dialect: &'static ::sql::Dialect,
        name: Option<&str>,
    ) -> Acknowledgement {
        let transport = Arc::clone(shared);
        accept::accepting(dialect, shared.accept.as_deref(), name, move |sql| {
            transport.exchange(&transport.server, &transport.database, |client| {
                W::execute(client, sql)
            })
        })
    }
}

impl<W: Wire> Transport for SqlTransport<W> {
    fn name(&self) -> &'static str {
        W::NAME
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn arrivals(&self) -> Arrivals {
        Arrivals::Ordered("a poll reads again what is not yet told")
    }

    fn waiting(&self) -> Waiting {
        Waiting::Polled("a receive runs the query and returns")
    }

    /// Run the query on the connection kept for the server and database,
    /// logged in on the first receive; each row is a Stream, whole. Its
    /// verdict runs the `accept` statement where one is declared — on
    /// `Accepted`, never on `Refused` or `Failed` — and tells the database
    /// nothing where none is: whether a row is read again is then the
    /// query's (a query that consumes as it reads makes acceptance
    /// at-most-once).
    fn receive_up_to(&self, most: usize) -> Result<Vec<Arrived>> {
        let (rows, dialect) = self.exchange(&self.server, &self.database, |client| {
            let rows = W::query(client, &self.query)?;
            Ok((rows, W::session_dialect(client).unwrap_or_else(W::dialect)))
        })?;
        let shared = Arc::new(self.clone());
        accept::arrivals(
            rows,
            most,
            &self.refused,
            |name| format!("{}://{}/{}?row={name}", W::NAME, self.server, self.database),
            Clone::clone,
            |value| self.column.bytes(&value, dialect),
            |name| Self::acknowledgement(&shared, dialect, name),
        )
    }

    fn sending(&self) -> Sending {
        Sending::Gathered("an INSERT carries the body as one literal in its statement")
    }

    /// Insert the bytes as one column of one row — in the dialect's binary
    /// form, or as text where the column is declared to hold it — on the
    /// connection kept for the server and database, logged in on the first
    /// send to them. A Stream that is not the column's text is refused
    /// before any connection; a session that reads another dialect has its
    /// literal written as it reads.
    fn send_from(&self, target: &str, outgoing: Outgoing<'_>) -> Result<()> {
        let bytes: &[u8] = &outgoing.gather()?;
        let to = destination(W::SCHEMES, target, &self.server, &self.database)?;
        let literal = self.column.literal(bytes, W::dialect())?;
        self.exchange(to.server, to.database, |client| {
            let insert = match W::session_dialect(client) {
                Some(dialect) => {
                    let literal = self.column.literal(bytes, dialect)?;
                    dialect.insert(to.table, to.column, &literal)
                }
                None => W::dialect().insert(to.table, to.column, &literal),
            };
            W::execute(client, &insert)
        })
    }

    /// Rows are artifacts, and one receive holds no transaction to claim
    /// one in.
    fn claims(&self) -> Option<&dyn ResourceClaim> {
        Some(&NoNativeClaim)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use ::sql::{Dialect, POSTGRESQL};
    use codec::unicode::Form;
    use xcore::settings::{Applies, Given, Settings};

    use super::*;
    use crate::acknowledgement::Refusal;
    use crate::configured::Configured;
    use crate::error::{TransportError, protocol_error};
    use crate::sql::declared::{DATABASE, QUERY, TIMEOUT, USER};
    use crate::sql::wire::Probe;
    use crate::sql::{COLUMN, ENCODING, Rows};

    /// A database in memory: the rows a query reads, and every statement
    /// run, in order.
    #[derive(Default)]
    struct Table {
        rows: Rows<String>,
        ran: Vec<String>,
    }

    /// A technology over [`Table`]: a login whose password is `wrong` is
    /// refused.
    #[derive(Clone, Default)]
    struct Memory(Arc<Mutex<Table>>);

    const SETTINGS: &Settings = &Settings {
        technology: "xmip-core-transport-memory",
        settings: &[
            DATABASE,
            USER,
            QUERY,
            accept::ACCEPT,
            COLUMN,
            ENCODING,
            TIMEOUT,
        ],
    };

    impl Wire for Memory {
        type Client = Arc<Mutex<Table>>;
        const NAME: &'static str = "memory";
        const SCHEMES: &'static [&'static str] = &["memory"];
        const SETTINGS: &'static Settings = SETTINGS;
        const PROBE: Probe = Probe {
            database: "probe",
            user: "xmip",
            password: "",
            target: "probe/payload",
        };

        fn dialect() -> &'static Dialect {
            &POSTGRESQL
        }

        fn connect(
            &self,
            _server: &str,
            _database: &str,
            login: &Login,
            _timeout: Option<Duration>,
        ) -> Result<Self::Client> {
            if login.password == "wrong" {
                return Err(TransportError::permanent("refused"));
            }
            Ok(Arc::clone(&self.0))
        }

        fn usable(_client: &mut Self::Client) -> bool {
            true
        }

        fn query(client: &mut Self::Client, sql: &str) -> Result<Rows<String>> {
            let mut table = client.lock().map_err(|_| protocol_error("poisoned"))?;
            table.ran.push(sql.to_string());
            Ok(table.rows.clone())
        }

        fn execute(client: &mut Self::Client, sql: &str) -> Result<()> {
            let mut table = client.lock().map_err(|_| protocol_error("poisoned"))?;
            table.ran.push(sql.to_string());
            Ok(())
        }

        fn take_one(_transport: &SqlTransport<Self>, _listener: &TcpListener) -> Result<Taken> {
            Err(protocol_error("no far end in memory"))
        }
    }

    fn row(name: Option<&str>, body: &str) -> Vec<Option<String>> {
        vec![name.map(str::to_string), Some(body.to_string())]
    }

    #[test]
    fn a_location_is_read_from_the_settings_every_sql_technology_declares() {
        assert_eq!(
            SqlTransport::<Memory>::SETTINGS.problems(),
            Vec::<String>::new()
        );
        let text = |name: &str, value: &str| (name.to_string(), Given::Text(value.to_string()));
        let given = [text("database", "orders"), text("user", "xmip")];
        let built = SqlTransport::<Memory>::open("db:1", Applies::Receive, &given).expect("built");
        assert_eq!((built.server(), built.database()), ("db:1", "orders"));
        assert_eq!(built.login().user, "xmip");
        assert_eq!(built.query(), DEFAULT_QUERY);
        assert_eq!(built.accept(), None, "nothing runs unless declared");
        assert_eq!(built.column(), Column::Binary, "bytes unless declared");
        assert_eq!(built.timeout(), None);
        let more = [
            text("database", "orders"),
            text("user", "xmip"),
            text("query", "SELECT id, body FROM outbox"),
            text("accept", "DELETE FROM outbox WHERE id = $1"),
            text("column", "text"),
            text("encoding", "utf-16le"),
            text("timeout", "2s"),
        ];
        let built = SqlTransport::<Memory>::open("db:1", Applies::Receive, &more).expect("built");
        assert_eq!(built.query(), "SELECT id, body FROM outbox");
        assert_eq!(built.accept(), Some("DELETE FROM outbox WHERE id = $1"));
        assert_eq!(built.column(), Column::Text(Form::Utf16Le));
        assert_eq!(built.timeout(), Some(Duration::from_secs(2)));
        let Err(refused) = SqlTransport::<Memory>::open("db:1", Applies::Send, &given[1..]) else {
            panic!("database is required");
        };
        assert!(refused.message.contains("\"database\""), "{refused}");
    }

    #[test]
    fn each_row_is_a_stream_and_only_an_accepted_one_is_consumed() {
        let memory = Memory::default();
        memory.0.lock().expect("lock").rows = vec![
            row(Some("41"), "\\x6f6b"),
            row(Some("it's"), "\\x6e6f"),
            row(Some("43"), "\\x6c61"),
            row(None, "\\x"),
        ];
        let near = SqlTransport::new("db:1", "orders", "xmip")
            .with_wire(memory.clone())
            .with_accept("DELETE FROM inbox WHERE id = $1");
        let mut arrived = near.receive().expect("rows");
        assert_eq!(arrived.len(), 4);
        assert_eq!(arrived[0].origin_uri, "memory://db:1/orders?row=41");
        assert_eq!(arrived[3].origin_uri, "memory://db:1/orders?row=3");
        assert_eq!(arrived.remove(0).taken().expect("accepted").bytes, b"ok");
        arrived
            .remove(0)
            .refused(Refusal::Unacceptable)
            .expect("left");
        arrived.remove(0).failed().expect("left");
        let unnamed = arrived.remove(0).taken().expect("unnamed");
        assert!(unnamed.bytes.is_empty(), "NULL is an empty Stream");
        let again = near.receive().expect("again");
        assert_eq!(again.len(), 3, "the refused row is not received again");
        drop(again);
        assert_eq!(
            memory.0.lock().expect("lock").ran,
            [
                DEFAULT_QUERY,
                "DELETE FROM inbox WHERE id = '41'",
                DEFAULT_QUERY
            ]
        );
        assert_eq!(near.opened(), 1, "one login for every statement");
    }

    #[test]
    fn a_send_inserts_one_literal_and_a_stream_not_the_columns_text_never_connects() {
        let memory = Memory::default();
        let near = SqlTransport::new("db:1", "orders", "xmip").with_wire(memory.clone());
        near.send("inbox/payload", b"it's").expect("binary");
        near.send("memory://db:2/other/outbox/body", b"")
            .expect("elsewhere");
        let text = near.clone().holding(Column::Text(Form::Utf8));
        text.send("inbox/payload", b"it's").expect("text");
        let refused = text.send("inbox/payload", &[0xff]).expect_err("not UTF-8");
        assert!(!refused.retryable);
        assert!(refused.message.contains("utf-8 text"), "{refused}");
        assert!(
            !near
                .send("only-one", b"x")
                .expect_err("no column")
                .retryable
        );
        assert_eq!(
            memory.0.lock().expect("lock").ran,
            [
                "INSERT INTO \"inbox\" (\"payload\") VALUES ('\\x69742773')",
                "INSERT INTO \"outbox\" (\"body\") VALUES ('\\x')",
                "INSERT INTO \"inbox\" (\"payload\") VALUES ('it''s')",
            ]
        );
        assert_eq!(near.opened(), 2, "one login per server and database");
        let wrong = near.with_password("wrong");
        assert!(
            wrong.connect().is_err(),
            "the login is the wire's to refuse"
        );
        assert_eq!(wrong.name(), "memory");
        assert!(wrong.claims().is_some(), "rows are artifacts");
    }
}
