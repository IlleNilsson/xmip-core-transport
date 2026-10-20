//! What every SQL transport shares: where a send target puts its row; the
//! one fixed table a session answers `SELECT` from; and what the payload
//! column holds, as a Location declares it. What a receive does with the
//! rows its query read — each an arrival, consumed by the Location's
//! `accept` statement after its cycle — is [`accept`].
//!
//! SQL itself — the dialect, the INSERT written and taken apart, a
//! literal, a statement's verb — is `xmip-core-library-sql`'s, one copy
//! for every domain (ADR-0044, amendment 2026-10-12); a technology names
//! the dialect it speaks and the URI schemes its Locations open with. Four
//! technologies carried the rest until 2026-09-14.
//!
//! Whether the column holds bytes or text is the Location's [`COLUMN`]
//! setting, and a text column's Unicode form its [`ENCODING`]: never the
//! bytes' own look (ADR-0038, amendment 2026-09-26): bytes that happen to
//! be UTF-8 are still bytes, and a value that happens to look like the
//! binary literal in a text column is still text.

use ::sql::{Dialect, Literal};
use codec::unicode::Form;
use net::Target;
use xcore::settings::{Applies, Fixed, Kind, Presence, Read, Setting};

use crate::error::{Result, TransportError};
use crate::taken::Taken;

pub mod accept;

/// The setting that says what the payload column holds.
pub const COLUMN: Setting = Setting {
    name: "column",
    kind: Kind::Choice {
        choices: &["binary", "text"],
    },
    presence: Presence::Default(Fixed::Text("binary")),
    meaning: "What the payload column holds: bytes, in the dialect's binary form, or text in \
              the declared encoding; binary when left out.",
    applies: Applies::Both,
};

/// The setting that names the Unicode form a text column's Stream is in.
pub const ENCODING: Setting = Setting {
    name: "encoding",
    kind: Kind::Choice {
        choices: &Form::WORDS,
    },
    presence: Presence::Default(Fixed::Text("utf-8")),
    meaning: "The Unicode form a Stream is in where the column holds text; utf-8 when left \
              out, and read only where column is text.",
    applies: Applies::Both,
};

/// What the one column a Location reads or inserts holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Column {
    /// Bytes, written and read in the dialect's binary form: the default.
    #[default]
    Binary,
    /// Text: a Stream in this Unicode form, decoded strictly on the way in
    /// and encoded to it on the way back.
    Text(Form),
}

impl Column {
    /// The column [`COLUMN`] and [`ENCODING`] declare.
    ///
    /// # Errors
    /// An encoding that names no Unicode form; the declaration refuses it
    /// before this is reached.
    pub fn configured(settings: &Read) -> Result<Self> {
        if settings.optional_text(COLUMN.name) != Some("text") {
            return Ok(Self::Binary);
        }
        let word = settings.optional_text(ENCODING.name).unwrap_or_default();
        Form::named(word)
            .map(Self::Text)
            .map_err(|error| TransportError::permanent(error.message))
    }

    /// The literal a send inserts `bytes` as: the dialect's binary literal
    /// for a binary column, its string literal for a text one.
    ///
    /// # Errors
    /// A text column's Stream that is not its declared form, or that holds
    /// a NUL, which no text column keeps.
    pub fn literal(self, bytes: &[u8], dialect: &Dialect) -> Result<String> {
        let Self::Text(form) = self else {
            return Ok(dialect.quote_bytes(bytes));
        };
        let text = form.decode(bytes).map_err(|error| {
            TransportError::permanent(format!(
                "the column holds {} text and this Stream is not: {error}",
                form.word()
            ))
        })?;
        if text.contains('\0') {
            return Err(TransportError::permanent(
                "the column holds text and this Stream carries a NUL, which no text column keeps",
            ));
        }
        Ok(dialect.quote_text(&text))
    }

    /// The bytes a value read back as text stands for: the bytes the
    /// dialect's binary form names, or the text in the declared form.
    ///
    /// # Errors
    /// A binary column's value that is not in the dialect's binary form.
    pub fn bytes(self, value: &str, dialect: &Dialect) -> Result<Vec<u8>> {
        match self {
            Self::Text(form) => Ok(form.encode(value)),
            Self::Binary => dialect.bytes_of(value).ok_or_else(|| {
                TransportError::permanent(
                    "the column holds bytes and the value read back is not in the dialect's \
                     binary form; a column that holds text is declared column = \"text\"",
                )
            }),
        }
    }

    /// The bytes a far end stores for `literal`: a binary literal's bytes
    /// as they are; a string's text in the declared form, or in UTF-8 — the
    /// connection's — where a binary column takes a string, as a database
    /// stores it.
    #[must_use]
    pub fn stored(self, literal: Literal) -> Vec<u8> {
        match (self, literal) {
            (_, Literal::Bytes(bytes)) => bytes,
            (Self::Text(form), Literal::Text(text)) => form.encode(&text),
            (Self::Binary, Literal::Text(text)) => text.into_bytes(),
        }
    }
}

/// Where a send puts its row: the server, the database, the table and the
/// column, as a target named them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destination<'a> {
    pub server: &'a str,
    pub database: &'a str,
    pub table: &'a str,
    pub column: &'a str,
}

/// Where `target` puts a row: `<scheme>://host:port/<database>/<table>
/// /<column>`, `host:port/<database>/<table>/<column>`, or
/// `<table>/<column>` on the configured `server` and `database`. `schemes`
/// are the ones a send target may open with: the technology's own and the
/// ones its users also write (`sqlserver://`, `postgres://`).
///
/// # Errors
/// A path that is not database/table/column or table/column, or that has
/// an empty segment.
pub fn destination<'a>(
    schemes: &[&str],
    target: &'a str,
    server: &'a str,
    database: &'a str,
) -> Result<Destination<'a>> {
    let refused = || {
        TransportError::permanent(format!(
            "{target:?} is not database/table/column or table/column"
        ))
    };
    let (server, named) = match Target::naming_server(schemes, target) {
        Some(named) => (named.authority(), named),
        None => (server, Target::relative(target)),
    };
    let segments = named.segments().map_err(|_| refused())?;
    let (database, table, column) = match segments.as_slice() {
        [database, table, column] => (*database, *table, *column),
        [table, column] => (database, *table, *column),
        _ => return Err(refused()),
    };
    if database.is_empty() {
        return Err(refused());
    }
    Ok(Destination {
        server,
        database,
        table,
        column,
    })
}

/// What a session answers a statement with before its fixed table is
/// consulted: `None` declines.
pub type Answering<A> = Box<dyn FnMut(&str) -> Option<A> + Send>;

/// The rows a session answers any SELECT with: a cell is a value or NULL.
pub type Rows<Cell> = Vec<Vec<Option<Cell>>>;

/// The columns and rows a session answers any SELECT with, owned.
#[must_use]
pub fn table<C: ?Sized + ToOwned>(
    columns: &[&str],
    rows: &[&[Option<&C>]],
) -> (Vec<String>, Rows<C::Owned>) {
    (
        columns.iter().map(ToString::to_string).collect(),
        rows.iter()
            .map(|row| row.iter().map(|v| v.map(ToOwned::to_owned)).collect())
            .collect(),
    )
}

/// The Stream a session reports as inserted, where the client inserted
/// one.
pub trait Inserted {
    /// The Stream, where this is an insert.
    fn inserted(self) -> Option<Taken>;
}

/// The next value the client inserts, or `None` when it closed; every
/// other event `next` reports is answered on the way and passed over.
///
/// # Errors
/// As `next`.
pub fn next_insert<E: Inserted>(
    mut next: impl FnMut() -> crate::error::Result<Option<E>>,
) -> crate::error::Result<Option<Taken>> {
    loop {
        match next()? {
            Some(event) => {
                if let Some(arrived) = event.inserted() {
                    return Ok(Some(arrived));
                }
            }
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ::sql::TSQL;

    const SCHEMES: &[&str] = &["example", "alias"];

    enum Event {
        Selected,
        Inserted(Taken),
    }

    impl Inserted for Event {
        fn inserted(self) -> Option<Taken> {
            match self {
                Self::Inserted(arrived) => Some(arrived),
                Self::Selected => None,
            }
        }
    }

    #[test]
    fn a_binary_column_takes_every_stream_as_bytes_even_utf8() {
        let column = Column::default();
        assert_eq!(column, Column::Binary);
        assert_eq!(
            column.literal(b"a row", &TSQL).expect("bytes"),
            "0x6120726f77"
        );
        assert_eq!(column.literal(&[0xff], &TSQL).expect("bytes"), "0xff");
        assert_eq!(column.bytes("0x6120", &TSQL).expect("binary form"), b"a ");
        let refused = column
            .bytes("a row", &TSQL)
            .expect_err("not the binary form");
        assert!(refused.message.contains("column = \"text\""), "{refused}");
        assert!(!refused.retryable);
        assert_eq!(column.stored(Literal::Text("é".into())), "é".as_bytes());
        assert_eq!(column.stored(Literal::Bytes(vec![0xff])), [0xff]);
    }

    #[test]
    fn a_text_column_decodes_strictly_in_its_form_and_refuses_the_rest() {
        let utf8 = Column::Text(Form::Utf8);
        assert_eq!(utf8.literal(b"a row", &TSQL).expect("text"), "N'a row'");
        assert_eq!(
            utf8.bytes("0x61", &TSQL).expect("text"),
            b"0x61",
            "text, not hex"
        );
        let refused = utf8.literal(&[0xff, 0xfe], &TSQL).expect_err("not UTF-8");
        assert!(refused.message.contains("utf-8"), "{refused}");
        assert!(utf8.literal(b"a\0row", &TSQL).is_err(), "a NUL");
        let utf16 = Column::Text(Form::Utf16Le);
        assert_eq!(utf16.literal(&[0x41, 0], &TSQL).expect("UTF-16LE"), "N'A'");
        assert_eq!(utf16.bytes("A", &TSQL).expect("encoded"), [0x41, 0]);
        assert_eq!(utf16.stored(Literal::Text("A".into())), [0x41, 0]);
        assert_eq!(utf16.stored(Literal::Bytes(vec![0xff])), [0xff]);
        assert!(utf16.literal(b"A", &TSQL).is_err(), "an odd length");
    }

    #[test]
    fn the_column_is_read_from_its_two_settings() {
        use xcore::settings::{Given, Settings};
        const DECLARED: &Settings = &Settings {
            technology: "xmip-core-transport-example",
            settings: &[COLUMN, ENCODING],
        };
        assert_eq!(DECLARED.problems(), Vec::<String>::new());
        let text = |name: &str, value: &str| (name.to_string(), Given::Text(value.to_string()));
        let column = |given: &[(String, Given)]| {
            let read = DECLARED.read(Applies::Send, given).expect("read");
            Column::configured(&read).expect("configured")
        };
        assert_eq!(column(&[]), Column::Binary);
        assert_eq!(column(&[text("column", "text")]), Column::Text(Form::Utf8));
        assert_eq!(
            column(&[text("column", "text"), text("encoding", "utf-16be")]),
            Column::Text(Form::Utf16Be)
        );
        assert_eq!(column(&[text("encoding", "utf-16be")]), Column::Binary);
        assert!(
            DECLARED
                .read(Applies::Send, &[text("encoding", "latin-1")])
                .is_err()
        );
    }

    #[test]
    fn a_target_is_read_once_for_every_scheme_the_dialect_names() {
        let at = |server, database, table, column| Destination {
            server,
            database,
            table,
            column,
        };
        let read = |target| destination(SCHEMES, target, "configured:1", "orders");
        assert_eq!(
            read("example://h:1/db/t/c").expect("scheme"),
            at("h:1", "db", "t", "c")
        );
        assert_eq!(
            read("alias://h:1/t/c").expect("alias"),
            at("h:1", "orders", "t", "c")
        );
        assert_eq!(read("h:2/db/t/c").expect("peer"), at("h:2", "db", "t", "c"));
        assert_eq!(
            read("t/c").expect("configured"),
            at("configured:1", "orders", "t", "c")
        );
        for bad in ["only-one", "a/b/c/d", "example://h:1/", "db//c", "t/"] {
            let refused = read(bad).expect_err(bad);
            assert!(!refused.retryable, "{bad}");
            assert!(
                refused.message.contains("database/table/column"),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_table_is_owned_and_the_next_insert_passes_other_events_over() {
        let (columns, rows) = table::<str>(&["id", "payload"], &[&[Some("1"), None]]);
        assert_eq!(columns, ["id", "payload"]);
        assert_eq!(rows, [[Some("1".to_string()), None]]);
        let (_, bytes) = table::<[u8]>(&["raw"], &[&[Some(&[1u8, 2][..])]]);
        assert_eq!(bytes, [[Some(vec![1u8, 2])]]);
        let mut events = vec![
            None,
            Some(Event::Inserted(Taken::new("sql://one", b"row"))),
            Some(Event::Selected),
        ];
        let mut next = || Ok(events.pop().flatten());
        let first = next_insert(&mut next).expect("insert").expect("one");
        assert_eq!(first.bytes, b"row");
        assert!(next_insert(&mut next).expect("closed").is_none());
    }
}
