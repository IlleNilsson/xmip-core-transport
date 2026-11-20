//! A transport built from a Location: its address, and its settings read
//! through the declaration the technology makes of them.
//!
//! ADR-0064, amendment 2026-09-26: a technology declares its own settings in
//! its own crate, and that declaration is what the technology reads its
//! settings through, validates a Location's TOML at start and fills the
//! developer's form. `Configured` is the transport's half of it: the
//! declaration as [`Configured::SETTINGS`], and the one constructor that
//! takes what the declaration read. The shape is `xcore::settings`, shared
//! with every other capability's technologies.
//!
//! A companion to [`Transport`] rather than a part of it:
//! `Transport` mirrors the C ABI's vtable (ADR-0010), and a declaration is
//! plain data a surface reads without building anything.
//!
//! A Location's `timeout`, how long its technology waits on a far end that
//! stops answering, is written here once (ADR-0064, amendment 2026-11-15):
//! declared through [`timeout`], read by [`Configured::open`] and handed on
//! through [`Configured::timing_out_after`] into the place
//! [`Configured::timeout_kept`] names. Until 2026-11-15 every technology
//! declared it, read it in its own `configured` and kept it through its own
//! builder.
//!
//! A Receive Location's listener is bound as its transport is opened, by
//! [`Configured::listening`], never on its first receive (the owner,
//! 2026-11-16, *Bind at open*): [`Configured::open`] builds and binds
//! nothing, so a surface validating a Location, or the runtime asking
//! whether one waits, takes no port.

use std::time::Duration;

use xcore::settings::{Applies, Fixed, Given, Kind, Presence, Read, Setting, Settings};

use crate::error::Result;
use crate::node::NodeLocation;
use crate::protocol::Transport;

/// The setting a Location bounds its technology's waits on a far end by.
pub const TIMEOUT: &str = "timeout";

/// The [`TIMEOUT`] setting as a technology declares it among its
/// [`Configured::SETTINGS`]: a duration, read on `applies`, meaning what
/// `meaning` says in the technology's words. A Location that leaves it out
/// waits `default` where there is one, and otherwise as the technology
/// waits when given none, which its `meaning` says.
#[must_use]
pub const fn timeout(
    applies: Applies,
    default: Option<Duration>,
    meaning: &'static str,
) -> Setting {
    Setting {
        name: TIMEOUT,
        kind: Kind::Duration,
        presence: match default {
            Some(default) => Presence::Default(Fixed::Duration(default)),
            None => Presence::Optional,
        },
        meaning,
        applies,
    }
}

/// What a technology keeps its timeout in: an `Option<Duration>`, waiting as
/// long as it takes until it is given one, or a `Duration`, its own default
/// until it is given another.
pub trait TimeoutKept {
    /// Waiting at most `timeout` from now on.
    fn keep(&mut self, timeout: Duration);
}

impl TimeoutKept for Option<Duration> {
    fn keep(&mut self, timeout: Duration) {
        *self = Some(timeout);
    }
}

impl TimeoutKept for Duration {
    fn keep(&mut self, timeout: Duration) {
        *self = timeout;
    }
}

/// A transport technology a Location can be configured with.
pub trait Configured: Transport + Sized {
    /// Every setting this technology takes beyond the Location's address —
    /// an empty list when it takes none — with `technology` its module name,
    /// `env!("CARGO_PKG_NAME")`.
    const SETTINGS: &'static Settings;

    /// The address schemes this technology answers, lowercase, at least one
    /// — `ftp` and `ftps`, `http` and `https`: the one place a Location's
    /// address is told to belong to it, the scheme it is entered with
    /// choosing the technology (ADR-0010 and ADR-0064, amendments
    /// 2026-10-28). No two technologies a node carries declare one alike.
    const SCHEMES: &'static [&'static str];

    /// `address`, as a Location enters it, in this technology's own terms:
    /// what follows `scheme://` by default, the address unchanged where it
    /// names no scheme. A technology that reads the scheme itself — the
    /// TLS `ftps` or `https` asks for, the user an `sftp://user@host`
    /// logs in as — keeps the whole address.
    #[must_use]
    fn own_terms(address: &str) -> &str {
        net::Target::under(Self::SCHEMES, address).map_or(address, |target| target.after_scheme())
    }

    /// The target every send from a Location of this technology at
    /// `address` names (the runtime's `Sending::target`). By default the
    /// address in its own terms ([`Configured::own_terms`]), for a
    /// technology whose address is where a send goes: a host and port, a
    /// URL, a path. A technology whose address names a server and whose
    /// settings name the destination on it — a broker and its topic, a
    /// namespace and its queue, a bus and its object — names
    /// [`crate::destination::OWN`], which its send reads as that configured
    /// destination ([`crate::destination::named`]), so a Send Location
    /// sends where its configuration says; a target naming another still
    /// wins (ADR-0064, amendment 2026-11-10).
    #[must_use]
    fn own_target(address: &str) -> &str {
        Self::own_terms(address)
    }

    /// This technology at `address`, in its own terms ([`Configured::own_terms`]),
    /// with the settings its declaration read.
    ///
    /// # Errors
    /// When the address or a setting is one the technology cannot use —
    /// never for a setting the declaration already refuses.
    fn configured(address: &str, settings: &Read) -> Result<Self>;

    /// Read what a Location on `side` gave through [`Configured::SETTINGS`],
    /// and build this technology from it, timing out after the Location's
    /// [`TIMEOUT`] where it read one ([`Configured::timing_out_after`]), on
    /// that side ([`Configured::sided`]): the one way from a Location's table
    /// to a transport.
    ///
    /// # Errors
    /// Every setting the declaration refuses, each naming the technology and
    /// the setting; then whatever [`Configured::configured`] refuses.
    fn open(address: &str, side: Applies, given: &[(String, Given)]) -> Result<Self> {
        let read = Self::SETTINGS.read(side, given)?;
        Ok(Self::configured(Self::own_terms(address), &read)?.finished(&read, side))
    }

    /// What [`Configured::open`] does with this technology once it is built
    /// from the settings `read`: the Location's [`TIMEOUT`] where it read
    /// one, and its side. The one writer of that step: a test that builds a
    /// technology on what its address cannot give it — a simulated bus where
    /// a build opens the kernel's — finishes it here too.
    #[must_use]
    fn finished(self, read: &Read, side: Applies) -> Self {
        let built = match read.optional_duration(TIMEOUT) {
            Some(timeout) => self.timing_out_after(timeout),
            None => self,
        };
        built.sided(side)
    }

    /// Where this technology keeps how long it waits on a far end that
    /// stops answering, its Location's [`TIMEOUT`]: what
    /// [`Configured::timing_out_after`] keeps a timeout in. `None`, as by
    /// default, for a technology that declares no timeout.
    fn timeout_kept(&mut self) -> Option<&mut dyn TimeoutKept> {
        None
    }

    /// This technology giving up on a far end that stops answering after
    /// `timeout`: what [`Configured::open`] hands it from its Location's
    /// [`TIMEOUT`], and what a loopback or a test builds it with. Kept where
    /// [`Configured::timeout_kept`] says; a technology that hands it on to a
    /// part it speaks through as well says so here.
    #[must_use]
    fn timing_out_after(mut self, timeout: Duration) -> Self {
        if let Some(kept) = self.timeout_kept() {
            kept.keep(timeout);
        }
        self
    }

    /// This transport on `side`, as [`Configured::open`] builds it. A
    /// technology whose address means another thing on each side keeps it
    /// — a file Location's is a drop directory to a receive and the file a
    /// send writes, which the node's start recovers in
    /// ([`Configured::on_node`]); any other keeps nothing, as by default.
    #[must_use]
    fn sided(self, _side: Applies) -> Self {
        self
    }

    /// This transport on the node at `node`: what the runtime calls once,
    /// as it builds a Location's transport, right after
    /// [`Configured::open`] (the owner's *Option A*, 2026-10-03: the runtime
    /// gives every transport its node's identity once, as it builds it,
    /// `node.rs`). A technology
    /// that records at the far end who holds what — a claimed file's name
    /// (ADR-0024, amendment 2026-09-26), an object's claim (amendment
    /// 2026-11-20) — keeps it, and returns there what the node held before
    /// it last stopped. Nothing is kept by default.
    ///
    /// # Errors
    /// Where the technology could not return what the node held before:
    /// the far end could not be read.
    fn on_node(self, _node: &NodeLocation) -> Result<Self> {
        Ok(self)
    }

    /// This transport holding its far ends to the keys `pins` keeps: what
    /// the runtime calls once, after [`Configured::on_node`], with the
    /// node's pins (`crate::pinned`). A technology that authenticates its
    /// far end by a key no anchor vouches for — an SSH server's host key —
    /// keeps them, pinning each far end's key on first contact (ADR-0019,
    /// amendment 2026-10-28); any other keeps nothing, as by default.
    #[must_use]
    fn pinned_by(self, _pins: &std::sync::Arc<crate::Pinned>) -> Self {
        self
    }

    /// This transport on what the Locations of its node share
    /// (`crate::sharing`): what the runtime calls once, after
    /// [`Configured::on_node`], with the node's sharing. A technology whose
    /// Locations name a device the operating system opens for one handle at
    /// a time — a serial port — takes the device's one port from it, so every
    /// Location naming the device on the node goes through that one
    /// (ADR-0064, amendment 2026-11-15); any other keeps nothing, as by
    /// default.
    #[must_use]
    fn shared_by(self, _sharing: &std::sync::Arc<dyn crate::Sharing>) -> Self {
        self
    }

    /// This transport presenting `secured`, the secret its Location names
    /// (`secret = "<name>"`; ADR-0019, amendment 2026-10-29): what the
    /// runtime calls once, after [`Configured::on_node`], for a Location
    /// that names one. A technology that authenticates to the party it
    /// connects to keeps it and reads it each time it connects
    /// (`crate::secured`); any other refuses it, as by default, so a
    /// Location naming a secret its technology cannot present is refused
    /// as the node starts.
    ///
    /// # Errors
    /// Where the technology presents no secret, or none from this side.
    fn secured_by(self, secured: crate::Secured) -> Result<Self> {
        Err(crate::TransportError::permanent(format!(
            "{} presents no secret, so a Location of it names none; '{}' is named",
            Self::SETTINGS.technology,
            secured.name()
        )))
    }

    /// This transport guarded by `guard`, the TLS the node terminates its
    /// receives with — `None` where the node configured no `receive`
    /// certificate (`crate::guard`): what the runtime calls once, after
    /// [`Configured::on_node`]. A technology whose Location's settings say
    /// it terminates TLS keeps it, and refuses to be built without one; any
    /// other keeps nothing, as by default.
    ///
    /// # Errors
    /// Where the Location asks for TLS and the node has no `receive`
    /// certificate to present.
    #[cfg(feature = "tls")]
    fn guarded_by(self, _guard: Option<&crate::guard::Guard>) -> Result<Self> {
        Ok(self)
    }

    /// This transport's receives giving each caller a turn of at most
    /// `turn`, the node's `[tuning] receive_turn`: what the runtime calls
    /// once, after [`Configured::on_node`]. A technology serving its
    /// callers through [`crate::serving::Serving`] hands it to its serving
    /// wait ([`crate::serving::Serving::turning_within`]), whose own
    /// default, [`crate::serving::TURN`], holds otherwise; any other keeps
    /// nothing, as by default.
    #[must_use]
    fn turning_within(self, _turn: Duration) -> Self {
        self
    }

    /// This transport telling time by `clock`, its node's: what the runtime
    /// calls once, after [`Configured::on_node`]. A technology that decides
    /// on a span counts it on the Clock's monotonic reading (ADR-0068,
    /// amendment 2026-11-15) — its serving wait's turn
    /// ([`crate::serving::Serving::timed_by`]), the frames an HTTP/2 peer
    /// sends in a span — so a test steps the Clock past it with no sleep;
    /// any other keeps nothing, as by default.
    #[must_use]
    fn timed_by(self, _clock: &std::sync::Arc<dyn xcore::Clock>) -> Self {
        self
    }

    /// Bind what this technology's receive waits on — its listener, its
    /// datagram socket, its pipe — where nothing is bound yet, and say
    /// where it listens: what the runtime calls once for each Receive
    /// Location, after [`Configured::guarded_by`], as it opens its transport
    /// at startup phase 9, and never for a Send Location (the owner,
    /// 2026-11-16, *Bind at open*).
    /// So a node listens when it says it started, and a port that is taken
    /// refuses its start there, the runtime naming the Location and the
    /// bind's failure the address and the port. A technology whose receive
    /// waits on what it keeps ([`crate::kept::Kept`],
    /// [`crate::serving::Serving`]) binds it here, through
    /// [`crate::kept::Kept::bound`], and its receive takes what this bound
    /// and binds nothing; any other listens on nothing, `None`, as by
    /// default — one that goes to look, or connects out.
    /// Until 2026-11-16 the first receive bound the listener, after the
    /// node had started (open problem 71).
    ///
    /// # Errors
    /// Where the listener could not be bound: its address taken, not this
    /// machine's, or not one the technology listens at.
    fn listening(&self) -> Result<Option<&str>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arrived::Arrived;
    use crate::direction::Directions;

    struct Example {
        address: String,
        topic: String,
        wait_ms: u128,
        timeout: Option<Duration>,
    }

    impl Transport for Example {
        fn name(&self) -> &'static str {
            "example"
        }
        fn directions(&self) -> Directions {
            Directions::BOTH
        }
        fn receive_up_to(&self, _most: usize) -> Result<Vec<Arrived>> {
            Ok(Vec::new())
        }
        fn arrivals(&self) -> crate::Arrivals {
            crate::Arrivals::Unordered("nothing ever arrives")
        }
        fn waiting(&self) -> crate::Waiting {
            crate::Waiting::Polled("it looks and finds nothing")
        }
        fn sending(&self) -> crate::Sending {
            crate::Sending::Streamed("nothing is held, as nothing is sent")
        }
        fn send_from(&self, _target: &str, _outgoing: crate::Outgoing<'_>) -> Result<()> {
            Ok(())
        }
    }

    impl Configured for Example {
        const SETTINGS: &'static Settings = &Settings {
            technology: "xmip-core-transport-example",
            settings: &[
                Setting {
                    name: "topic",
                    kind: Kind::Text,
                    presence: Presence::Required,
                    meaning: "The topic a Location reads or writes.",
                    applies: Applies::Both,
                },
                Setting {
                    name: "wait",
                    kind: Kind::Duration,
                    presence: Presence::Default(Fixed::Text("5s")),
                    meaning: "How long one receive waits.",
                    applies: Applies::Receive,
                },
                timeout(
                    Applies::Both,
                    None,
                    "How long a far end that stops answering is waited on; unbounded when left \
                     out.",
                ),
            ],
        };

        const SCHEMES: &'static [&'static str] = &["example"];

        fn configured(address: &str, settings: &Read) -> Result<Self> {
            Ok(Self {
                address: address.to_string(),
                topic: settings.text("topic").to_string(),
                wait_ms: settings
                    .optional_duration("wait")
                    .map_or(0, |wait| wait.as_millis()),
                timeout: None,
            })
        }

        fn timeout_kept(&mut self) -> Option<&mut dyn TimeoutKept> {
            Some(&mut self.timeout)
        }
    }

    /// A Location for the topic `orders`, with `more` beside it.
    fn orders(more: &[(&str, Given)]) -> Vec<(String, Given)> {
        let topic = ("topic".to_string(), Given::Text("orders".to_string()));
        let more = more
            .iter()
            .map(|(name, value)| (name.to_string(), value.clone()));
        std::iter::once(topic).chain(more).collect()
    }

    #[test]
    fn a_locations_configured_timeout_reaches_the_technology() {
        assert_eq!(Example::SETTINGS.problems(), Vec::<String>::new());
        let given = orders(&[(TIMEOUT, Given::Text("250ms".to_string()))]);
        for side in [Applies::Receive, Applies::Send] {
            let built = Example::open("host:1", side, &given).expect("built");
            assert_eq!(built.timeout, Some(Duration::from_millis(250)), "{side:?}");
        }
        let unset = Example::open("host:1", Applies::Receive, &orders(&[])).expect("built");
        assert_eq!(
            unset.timeout, None,
            "a Location that sets none waits unbounded"
        );
        assert_eq!(
            unset.listening().expect("nothing to bind"),
            None,
            "a technology that keeps no listener listens on nothing"
        );

        let wrong = orders(&[(TIMEOUT, Given::Integer(250))]);
        let Err(refused) = Example::open("host:1", Applies::Receive, &wrong) else {
            panic!("a timeout is a duration");
        };
        assert!(
            refused.message.contains("\"timeout\""),
            "{}",
            refused.message
        );
    }

    #[test]
    fn a_declared_default_is_what_a_location_that_leaves_the_timeout_out_waits() {
        const DEFAULTED: &Settings = &Settings {
            technology: "xmip-core-transport-example",
            settings: &[timeout(
                Applies::Receive,
                Some(Duration::from_secs(2)),
                "How long a receive waits for a frame before it finds none.",
            )],
        };
        assert_eq!(DEFAULTED.problems(), Vec::<String>::new());
        let declared = DEFAULTED.setting(TIMEOUT).expect("declared");
        assert_eq!(declared.kind, Kind::Duration);
        let default = Fixed::Duration(Duration::from_secs(2));
        assert_eq!(declared.presence, Presence::Default(default));
        let received = DEFAULTED.read(Applies::Receive, &[]).expect("read");
        assert_eq!(
            received.optional_duration(TIMEOUT),
            Some(Duration::from_secs(2))
        );
        let sent = DEFAULTED.read(Applies::Send, &[]).expect("read");
        assert_eq!(
            sent.optional_duration(TIMEOUT),
            None,
            "the Send side waits on nothing"
        );
    }

    #[test]
    fn a_timeout_is_kept_as_its_technology_keeps_it() {
        let mut unbounded: Option<Duration> = None;
        unbounded.keep(Duration::from_millis(250));
        assert_eq!(unbounded, Some(Duration::from_millis(250)));
        let mut defaulted = Duration::from_secs(1);
        defaulted.keep(Duration::from_millis(250));
        assert_eq!(defaulted, Duration::from_millis(250));
    }

    #[test]
    fn a_location_is_read_through_the_declaration_and_built() {
        let given = [("topic".to_string(), Given::Text("orders".to_string()))];
        let built = Example::open("host:1", Applies::Receive, &given).expect("built");
        assert_eq!(built.topic, "orders");
        assert_eq!(built.wait_ms, 5000);

        let sent = Example::open("host:1", Applies::Send, &given).expect("built");
        assert_eq!(sent.wait_ms, 0, "a Send Location reads no receive setting");
    }

    #[test]
    fn the_scheme_an_address_is_entered_with_is_off_it_in_the_technologys_terms() {
        let given = [("topic".to_string(), Given::Text("orders".to_string()))];
        let built = Example::open("EXAMPLE://host:1/orders", Applies::Send, &given);
        assert_eq!(built.expect("built").address, "host:1/orders");
        let bare = Example::open("host:1", Applies::Send, &given).expect("built");
        assert_eq!(bare.address, "host:1", "an address with no scheme is kept");
    }

    #[test]
    fn a_send_names_its_address_in_its_own_terms_unless_the_technology_says_otherwise() {
        assert_eq!(
            Example::own_target("EXAMPLE://host:1/orders"),
            "host:1/orders"
        );
        assert_eq!(Example::own_target("host:1"), "host:1");
    }

    #[test]
    fn a_refused_setting_is_a_permanent_failure_naming_it() {
        let Err(refused) = Example::open("host:1", Applies::Receive, &[]) else {
            panic!("topic is required");
        };
        assert!(!refused.retryable);
        assert!(refused.message.contains("\"topic\""), "{}", refused.message);
        assert!(refused.message.contains("xmip-core-transport-example"));
    }
}
