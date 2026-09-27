//! What every SQL transport's far end shares: the one statement a
//! `Transport::send` writes, taken apart so a session can record what a
//! client inserted without being a SQL parser; the verb of a statement;
//! the one fixed table a session answers SELECTs from; and what the
//! payload column holds, as a Location declares it.
//!
//! Which delimiters, the bytes literal and the escapes are the dialect
//! (ADR-0044 clause 2): each technology keeps its own and hands them in,
//! reading and writing its delimited text through `codec::sql`. Four
//! technologies carried the rest until 2026-09-14.
//!
//! Whether the column holds bytes or text is the Location's [`COLUMN`]
//! setting, and a text column's Unicode form its [`ENCODING`]: never the
//! bytes' own look (ADR-0038, amendment 2026-09-26): bytes that happen to
//! be UTF-8 are still bytes, and a value that happens to look like the
//! binary literal in a text column is still text.

use codec::unicode::Form;
use xcore::settings::{Applies, Fixed, Kind, Presence, Read, Setting};

use crate::arrived::Arrived;
use crate::error::{Result, TransportError};

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

    /// The literal a send inserts `bytes` as: `binary` writes the dialect's
    /// binary literal, `quote` its string literal.
    ///
    /// # Errors
    /// A text column's Stream that is not its declared form, or that holds
    /// a NUL, which no text column keeps.
    pub fn literal(
        self,
        bytes: &[u8],
        quote: impl FnOnce(&str) -> String,
        binary: impl FnOnce(&[u8]) -> String,
    ) -> Result<String> {
        let Self::Text(form) = self else {
            return Ok(binary(bytes));
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
        Ok(quote(&text))
    }

    /// The bytes a value read back as text stands for: what `binary` reads
    /// from the dialect's binary form, or the text in the declared form.
    ///
    /// # Errors
    /// A binary column's value that is not in the dialect's binary form.
    pub fn bytes(
        self,
        value: &str,
        binary: impl FnOnce(&str) -> Option<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        match self {
            Self::Text(form) => Ok(form.encode(value)),
            Self::Binary => binary(value).ok_or_else(|| {
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

/// One literal a far end read from an INSERT, in a dialect whose binary
/// literal is its own syntax rather than a string: `X'…'`, `0x…`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    /// The bytes a binary literal names.
    Bytes(Vec<u8>),
    /// A string literal's text, its escapes undone.
    Text(String),
}

/// The verb a statement opens with, upper-cased: `SELECT`, `INSERT`.
#[must_use]
pub fn verb(sql: &str) -> String {
    sql.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase()
}

/// `rest` after `word`, matched without regard to case; `None` where
/// `rest` does not open with it.
#[must_use]
pub fn strip_word<'a>(rest: &'a str, word: &str) -> Option<&'a str> {
    let rest = rest.trim_start();
    let head = rest.get(..word.len())?;
    head.eq_ignore_ascii_case(word).then(|| &rest[word.len()..])
}

/// `INSERT INTO <table> (<column>) VALUES (<literal>)` taken apart: the
/// table, the column and the literal's value. `identifier` reads one
/// identifier as the dialect quotes it and `literal` one literal as the
/// dialect writes it, each returning what follows; anything else, or more
/// than one column, is `None`.
#[must_use]
pub fn parse_insert<'a, V>(
    sql: &'a str,
    identifier: impl Fn(&'a str) -> Option<(String, &'a str)>,
    literal: impl Fn(&'a str) -> Option<(V, &'a str)>,
) -> Option<(String, String, V)> {
    let rest = sql.trim().trim_end_matches(';');
    let rest = strip_word(rest, "INSERT")?;
    let rest = strip_word(rest, "INTO")?;
    let (table, rest) = identifier(rest)?;
    let rest = rest.trim_start().strip_prefix('(')?;
    let (column, rest) = identifier(rest)?;
    let rest = rest.trim_start().strip_prefix(')')?;
    let rest = strip_word(rest, "VALUES")?;
    let rest = rest.trim_start().strip_prefix('(')?.trim_start();
    let (value, tail) = literal(rest)?;
    (tail.trim() == ")").then_some((table, column, value))
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
    fn inserted(self) -> Option<Arrived>;
}

/// The next value the client inserts, or `None` when it closed; every
/// other event `next` reports is answered on the way and passed over.
///
/// # Errors
/// As `next`.
pub fn next_insert<E: Inserted>(
    mut next: impl FnMut() -> crate::error::Result<Option<E>>,
) -> crate::error::Result<Option<Arrived>> {
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

    fn bare(rest: &str) -> Option<(String, &str)> {
        let rest = rest.trim_start();
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        (end > 0).then(|| (rest[..end].to_string(), &rest[end..]))
    }

    fn quoted(rest: &str) -> Option<(String, &str)> {
        codec::sql::Delimiter::STRING.unquote_prefix(rest).ok()
    }

    enum Event {
        Selected,
        Inserted(Arrived),
    }

    impl Inserted for Event {
        fn inserted(self) -> Option<Arrived> {
            match self {
                Self::Inserted(arrived) => Some(arrived),
                Self::Selected => None,
            }
        }
    }

    fn quote(text: &str) -> String {
        format!("'{text}'")
    }

    fn hex(bytes: &[u8]) -> String {
        format!("0x{}", codec::hex::encode(bytes))
    }

    fn unhex(text: &str) -> Option<Vec<u8>> {
        codec::hex::decode(text.strip_prefix("0x")?).ok()
    }

    #[test]
    fn a_binary_column_takes_every_stream_as_bytes_even_utf8() {
        let column = Column::default();
        assert_eq!(column, Column::Binary);
        assert_eq!(
            column.literal(b"a row", quote, hex).expect("bytes"),
            "0x6120726f77"
        );
        assert_eq!(column.literal(&[0xff], quote, hex).expect("bytes"), "0xff");
        assert_eq!(column.bytes("0x6120", unhex).expect("binary form"), b"a ");
        let refused = column
            .bytes("a row", unhex)
            .expect_err("not the binary form");
        assert!(refused.message.contains("column = \"text\""), "{refused}");
        assert!(!refused.retryable);
        assert_eq!(column.stored(Literal::Text("é".into())), "é".as_bytes());
        assert_eq!(column.stored(Literal::Bytes(vec![0xff])), [0xff]);
    }

    #[test]
    fn a_text_column_decodes_strictly_in_its_form_and_refuses_the_rest() {
        let utf8 = Column::Text(Form::Utf8);
        assert_eq!(utf8.literal(b"a row", quote, hex).expect("text"), "'a row'");
        assert_eq!(
            utf8.bytes("0x61", unhex).expect("text"),
            b"0x61",
            "text, not hex"
        );
        let refused = utf8
            .literal(&[0xff, 0xfe], quote, hex)
            .expect_err("not UTF-8");
        assert!(refused.message.contains("utf-8"), "{refused}");
        assert!(utf8.literal(b"a\0row", quote, hex).is_err(), "a NUL");
        let utf16 = Column::Text(Form::Utf16Le);
        assert_eq!(
            utf16.literal(&[0x41, 0], quote, hex).expect("UTF-16LE"),
            "'A'"
        );
        assert_eq!(utf16.bytes("A", unhex).expect("encoded"), [0x41, 0]);
        assert_eq!(utf16.stored(Literal::Text("A".into())), [0x41, 0]);
        assert_eq!(utf16.stored(Literal::Bytes(vec![0xff])), [0xff]);
        assert!(utf16.literal(b"A", quote, hex).is_err(), "an odd length");
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
    fn an_insert_of_one_column_is_taken_apart_with_the_dialect_handed_in() {
        assert_eq!(
            parse_insert(
                "insert into inbox ( payload ) values ( 'x' );",
                bare,
                quoted
            ),
            Some(("inbox".into(), "payload".into(), "x".into()))
        );
        assert!(parse_insert("INSERT INTO inbox (a, b) VALUES ('x', 'y')", bare, quoted).is_none());
        assert!(parse_insert("INSERT INTO inbox (a) VALUES ('open", bare, quoted).is_none());
        assert!(parse_insert("UPDATE inbox SET a = 'x'", bare, quoted).is_none());
        assert_eq!(strip_word("  Values (", "VALUES"), Some(" ("));
        assert_eq!(strip_word("VAL", "VALUES"), None);
        assert_eq!(verb("  select 1"), "SELECT");
        assert_eq!(verb(""), "");
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
            Some(Event::Inserted(Arrived::new("sql://one", b"row"))),
            Some(Event::Selected),
        ];
        let mut next = || Ok(events.pop().flatten());
        let first = next_insert(&mut next).expect("insert").expect("one");
        assert_eq!(first.bytes, b"row");
        assert!(next_insert(&mut next).expect("closed").is_none());
    }
}
