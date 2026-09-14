//! What every SQL transport's far end shares: the one statement a
//! `Transport::send` writes, taken apart so a session can record what a
//! client inserted without being a SQL parser; the verb of a statement;
//! the one fixed table a session answers SELECTs from; and what can be a
//! text column at all.
//!
//! The quoting, the bytes literal and the escapes are the dialect
//! (ADR-0044 clause 2): each technology keeps its own and hands them in.
//! Four technologies carried the rest until 2026-09-14.

use crate::arrived::Arrived;

/// True when `bytes` can be one text column: UTF-8 without a NUL.
#[must_use]
pub fn is_text(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_ok_and(|text| !text.contains('\0'))
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
        let inner = rest.strip_prefix('\'')?;
        let end = inner.find('\'')?;
        Some((inner[..end].to_string(), &inner[end + 1..]))
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

    #[test]
    fn text_is_utf8_without_a_nul() {
        assert!(is_text(b"a row"));
        assert!(is_text(b""));
        assert!(!is_text(b"a\0row"));
        assert!(!is_text(&[0xff, 0xfe]));
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
