//! Where a send goes on a technology whose Location names a server by its
//! address and the destination on it by a setting: a broker and its topic,
//! a server and its subject, queue, stream or mailbox (ADR-0064, amendment
//! 2026-11-10).
//!
//! A Send Location of such a technology names [`OWN`], the empty target
//! (`Configured::own_target`): the destination its settings name, on the
//! server its address names. A target naming another still wins: the
//! server and the destination both, the server alone and the configured
//! destination on it, or the destination alone on the configured server.
//!
//! The empty target is read here once, by [`named`]: every technology that
//! takes it as what its Location configured — a topic, a queue, an object,
//! a Party's endpoint — asks it, [`on_server`] among them.

use net::Target;

/// The empty target: the destination the Location configured, on the
/// server its address names. What a Send Location of a technology whose
/// settings name its destination sends to (`Configured::own_target`).
pub const OWN: &str = "";

/// `target` where it names a destination, `None` where it is [`OWN`] and
/// the send goes where the Location is configured to: the one reading of
/// the empty target.
#[must_use]
pub fn named(target: &str) -> Option<&str> {
    (target != OWN).then_some(target)
}

/// Where a send to `target` goes, as `(server, destination)`, on a
/// technology configured with `own`, its server and its destination:
/// `own` for [`OWN`]; the server and destination `target` names; the
/// server it names and the configured destination, where it names no
/// destination; or the configured server and `target` as the destination,
/// where it names no server. `read` is `target` as the technology reads
/// one naming a server — `Target::under` its schemes, or
/// `Target::naming_server` where it also takes `host:port/destination`
/// without a scheme.
#[must_use]
pub fn on_server<'a>(
    target: &'a str,
    read: Option<Target<'a>>,
    own: (&'a str, &'a str),
) -> (&'a str, &'a str) {
    let (server, destination) = own;
    let Some(target) = named(target) else {
        return own;
    };
    match read {
        Some(read) => (read.authority(), named(read.path()).unwrap_or(destination)),
        None => (server, target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMES: &[&str] = &["broker"];
    const OWN_SERVER: (&str, &str) = ("host:1", "orders");

    fn under(target: &str) -> (&str, &str) {
        on_server(target, Target::under(SCHEMES, target), OWN_SERVER)
    }

    #[test]
    fn only_the_empty_target_names_no_destination() {
        assert_eq!(named(OWN), None);
        assert_eq!(named("invoices"), Some("invoices"));
        assert_eq!(
            named(" "),
            Some(" "),
            "a blank is a name, not the empty target"
        );
    }

    #[test]
    fn the_empty_target_is_the_configured_destination_on_the_configured_server() {
        assert_eq!(under(OWN), OWN_SERVER);
        let bare = on_server(OWN, Target::naming_server(SCHEMES, OWN), OWN_SERVER);
        assert_eq!(bare, OWN_SERVER);
    }

    #[test]
    fn a_target_naming_another_wins() {
        assert_eq!(under("invoices"), ("host:1", "invoices"));
        assert_eq!(under("BROKER://other:2"), ("other:2", "orders"));
        assert_eq!(under("broker://other:2/invoices"), ("other:2", "invoices"));
        let bare = "other:2/invoices";
        let read = on_server(bare, Target::naming_server(SCHEMES, bare), OWN_SERVER);
        assert_eq!(read, ("other:2", "invoices"));
    }

    #[test]
    fn the_address_without_its_scheme_is_a_destination_alone() {
        // What the runtime named until 2026-11-10, and why a Send Location
        // names `OWN` instead: `host:1` reads as a destination of that name.
        assert_eq!(under("host:1"), ("host:1", "host:1"));
    }
}
