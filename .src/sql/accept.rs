//! What a SQL Receive Location does with the rows its query read: each row
//! an arrival, and the statement that consumes it once its cycle has ended.
//!
//! A query that consumes as it reads — a `DELETE … RETURNING` — consumes
//! before the receive cycle has run, so a cycle that fails loses the row.
//! The [`ACCEPT`] setting moves the consuming act after the cycle: the
//! query only reads, and `accept` — `DELETE FROM inbox WHERE id = $1`, an
//! `UPDATE` of a status column — runs on the row's verdict with the row's
//! name, the query's first column as the origin carries it, in place of
//! the dialect's first parameter ([`super::Dialect::marker`]). The name
//! goes in as a string literal the technology writes with its own quoting,
//! never as text spliced raw: whatever a row's first column holds stays
//! one value. It runs on `Accepted`, and on `Refused` too — a table has no
//! place for a refused row, the runtime audited the refusal, and from
//! Message creation on the Stream is kept in Xmip (ADR-0013) — never on
//! `Failed`, which leaves the row for the next receive.

use codec::sql::Delimiter;
use xcore::settings::{Applies, Kind, Presence, Setting};

use super::Dialect;
use crate::acknowledgement::{Acknowledgement, Verdict};
use crate::arrived::Arrived;
use crate::error::Result;

/// The setting that names the statement run on a row's verdict.
pub const ACCEPT: Setting = Setting {
    name: "accept",
    kind: Kind::Text,
    presence: Presence::Optional,
    meaning: "A statement run once a row's receive cycle accepted or refused it, the row's name \
              (the query's first column) in place of the dialect's first parameter: a DELETE or \
              UPDATE that consumes the row only after its Stream is Xmip's, or refused for good. \
              A row whose cycle failed is left for the next receive. Nothing runs when left out, \
              and the query alone decides whether a row is read again.",
    applies: Applies::Receive,
};

impl Dialect {
    /// `statement` with every [`Dialect::marker`] outside its string
    /// literals and quoted identifiers replaced by `literal`; a marker
    /// followed by a digit (`$10` for `$1`) is another parameter and stays.
    #[must_use]
    pub fn bound(&self, statement: &str, literal: &str) -> String {
        let mut written = String::with_capacity(statement.len() + literal.len());
        let mut rest = statement;
        while let Some(next) = rest.chars().next() {
            let quoted = [Delimiter::STRING, self.identifier]
                .into_iter()
                .find(|delimiter| delimiter.open == next)
                .and_then(|delimiter| delimiter.unquote_prefix(rest).ok());
            if let Some((_, after)) = quoted {
                written.push_str(&rest[..rest.len() - after.len()]);
                rest = after;
                continue;
            }
            if let Some(after) = rest.strip_prefix(self.marker)
                && !after.starts_with(|c: char| c.is_ascii_digit())
            {
                written.push_str(literal);
                rest = after;
                continue;
            }
            written.push(next);
            rest = &rest[next.len_utf8()..];
        }
        written
    }

    /// The acknowledgement of the row `name`: `accept`, the name bound in
    /// as the literal `quote` writes, handed to `run` on `Accepted` and
    /// `Refused`; nothing on `Failed`, nor where there is no statement or
    /// no name to bind.
    #[must_use]
    pub fn accepting(
        &self,
        accept: Option<&str>,
        name: Option<&str>,
        quote: impl FnOnce(&str) -> String,
        run: impl FnOnce(&str) -> Result<()> + Send + 'static,
    ) -> Acknowledgement {
        let (Some(accept), Some(name)) = (accept, name) else {
            return Acknowledgement::unconsumed();
        };
        let statement = self.bound(accept, &quote(name));
        Acknowledgement::deferred(move |verdict| match verdict {
            Verdict::Accepted | Verdict::Refused(_) => run(&statement),
            Verdict::Failed => Ok(()),
        })
    }
}

/// The rows a query read, each one Stream, whole: its name the first
/// column as `name` reads it — its index where that is NULL — its origin
/// what `origin` makes of that name, its body the last column as `bytes`
/// reads it — empty where that is NULL — and its acknowledgement what
/// `acknowledgement` makes of the name the row has, if any.
///
/// # Errors
/// Where `bytes` refuses a value.
pub fn arrivals<V>(
    rows: Vec<Vec<Option<V>>>,
    origin: impl Fn(&str) -> String,
    name: impl Fn(&V) -> String,
    bytes: impl Fn(V) -> Result<Vec<u8>>,
    acknowledgement: impl Fn(Option<&str>) -> Acknowledgement,
) -> Result<Vec<Arrived>> {
    let mut arrived = Vec::with_capacity(rows.len());
    for (index, mut row) in rows.into_iter().enumerate() {
        let named = row.first().and_then(Option::as_ref).map(&name);
        let body = match row.pop().flatten() {
            Some(value) => bytes(value)?,
            None => Vec::new(),
        };
        let told = acknowledgement(named.as_deref());
        let name = named.unwrap_or_else(|| index.to_string());
        arrived.push(Arrived::whole(origin(&name), body, told));
    }
    Ok(arrived)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::acknowledgement::Refusal;

    const DIALECT: Dialect = Dialect {
        schemes: &["example"],
        catalog: "database",
        identifier: Delimiter::IDENTIFIER,
        bare: &['_'],
        marker: "$1",
    };

    fn quote(text: &str) -> String {
        Delimiter::STRING.quote(text)
    }

    #[test]
    fn the_marker_is_replaced_by_the_literal_where_it_is_a_parameter() {
        assert_eq!(
            DIALECT.bound("DELETE FROM inbox WHERE id = $1", "'41'"),
            "DELETE FROM inbox WHERE id = '41'"
        );
        assert_eq!(
            DIALECT.bound("UPDATE inbox SET done = $1 WHERE id = $1", "'x'"),
            "UPDATE inbox SET done = 'x' WHERE id = 'x'"
        );
        let question = Dialect {
            marker: "?",
            ..DIALECT
        };
        assert_eq!(
            question.bound("DELETE FROM t WHERE id = ? AND note <> '?'", "'7'"),
            "DELETE FROM t WHERE id = '7' AND note <> '?'"
        );
    }

    #[test]
    fn a_marker_in_a_literal_a_quoted_name_or_a_longer_parameter_stays() {
        assert_eq!(
            DIALECT.bound(
                "UPDATE \"t$1\" SET note = 'cost $1', n = $10 WHERE id = $1",
                "'7'"
            ),
            "UPDATE \"t$1\" SET note = 'cost $1', n = $10 WHERE id = '7'"
        );
        assert_eq!(
            DIALECT.bound("DELETE FROM inbox", "'7'"),
            "DELETE FROM inbox"
        );
    }

    #[test]
    fn accept_runs_on_accepted_and_refused_never_on_failed_nor_unnamed() {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let told = |name: Option<&str>, verdict| {
            let into = Arc::clone(&ran);
            DIALECT
                .accepting(
                    Some("DELETE FROM t WHERE id = $1"),
                    name,
                    quote,
                    move |sql| {
                        into.lock().expect("lock").push(sql.to_string());
                        Ok(())
                    },
                )
                .acknowledge(verdict)
                .expect("told");
        };
        told(Some("it's"), Verdict::Accepted);
        told(Some("2"), Verdict::Refused(Refusal::Unacceptable));
        told(Some("3"), Verdict::Failed);
        told(None, Verdict::Accepted);
        DIALECT
            .accepting(None, Some("4"), quote, |_| panic!("no statement"))
            .acknowledge(Verdict::Accepted)
            .expect("nothing to run");
        assert_eq!(
            *ran.lock().expect("lock"),
            [
                "DELETE FROM t WHERE id = 'it''s'",
                "DELETE FROM t WHERE id = '2'"
            ]
        );
    }

    #[test]
    fn each_row_is_a_stream_named_by_its_first_column_or_its_index() {
        let rows = vec![
            vec![Some("41".to_string()), Some("order".to_string())],
            vec![Some("42".to_string()), None],
            vec![None, Some("unnamed".to_string())],
        ];
        let named = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&named);
        let arrived = arrivals(
            rows,
            |name| format!("example://db?row={name}"),
            Clone::clone,
            |value| Ok(value.into_bytes()),
            move |name| {
                seen.lock().expect("lock").push(name.map(str::to_string));
                Acknowledgement::unconsumed()
            },
        )
        .expect("rows");
        let taken: Vec<_> = arrived
            .into_iter()
            .map(|one| one.taken().expect("taken"))
            .collect();
        assert_eq!(taken[0].origin_uri, "example://db?row=41");
        assert_eq!(taken[0].bytes, b"order");
        assert!(taken[1].bytes.is_empty(), "NULL is an empty Stream");
        assert_eq!(taken[2].origin_uri, "example://db?row=2");
        assert_eq!(
            *named.lock().expect("lock"),
            [Some("41".to_string()), Some("42".to_string()), None]
        );
        assert!(
            arrivals(
                vec![vec![Some(1u8)]],
                str::to_string,
                ToString::to_string,
                |_| Err(crate::error::TransportError::permanent("refused")),
                |_| Acknowledgement::unconsumed(),
            )
            .is_err()
        );
    }
}
