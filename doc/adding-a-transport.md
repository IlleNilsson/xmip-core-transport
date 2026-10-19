# Adding a transport

Moved here from the estate root on 2026-09-12 (ADR-0020 clause 3: the document
lives where its subject lives).

A transport is a technology of this capability: its repository is
`xmip-core-transport-<name>`, declared under `[xmip.core.transport.<name>]` in
`architecture.toml`, mounted at `<name>` directly inside this repository
(ADR-0016, amended 2026-09-07), and it depends on this crate, never the
reverse. How a repository is named, declared, created, mounted and landed is
the estate's and is not repeated here: `doc/architecture/repository-model.md`
sections 2, 7, 8 and 10. What follows is what a transport adds to that.

## The base you implement

`Transport` (`.src/protocol.rs`) — and nothing in it names a protocol:

```rust
pub trait Transport {
    fn name(&self) -> &'static str;                          // the token in the repo name
    fn directions(&self) -> Directions;                      // receive, send, or both
    fn receive_up_to(&self, most: usize) -> Result<Vec<Arrived>>; // at most `most`; the rest stay
    fn arrivals(&self) -> Arrivals;                          // told in order, or each alone
    fn waiting(&self) -> Waiting;                            // waits for an arrival, or polled
    fn sending(&self) -> Sending;                            // streams the body, or gathers it
    fn send_from(&self, target: &str, outgoing: Outgoing<'_>) -> Result<()>;
    fn claims(&self) -> Option<&dyn ResourceClaim> { None }  // exactly-once pickup (ADR-0024)
    // provided: receive() is receive_up_to without a bound, send(target, bytes)
    // is send_from over bytes held in memory — what a test or a tool asks
}
```

`receive_up_to` is how the runtime receives: `most` is the room its
Location's pool and the node have, one at least. Take no more; leave the
rest at the far end — a file unclaimed, a message on its broker, a row in
its table — for a later receive. The shared machinery takes the bound for
you: the file family's claim (`claim::Claiming::take`), an object store's
listing (`listed::listed`), a SQL query's rows (`sql::accept::arrivals`), a
kept subscription drained (`pool::delivered`). A protocol that hands over
one arrival a receive takes one whatever `most` says.

`send_from` is how the runtime sends: `outgoing` is the Message as the
Ledger keeps it, read a chunk at a time, its length known before its first
byte (`.src/outgoing.rs`), and its key (`Outgoing::key`), the Journey's
identifier, the deduplication key (runtime-model section 15, *Delivery
semantics*). Write the body on as it is read where the protocol allows —
`Outgoing::write_to` into a socket, a file, a data connection, or
`Outgoing::pieces` for requests of the protocol's own size, an SFTP, SMB or
NFS `WRITE` — and declare `Sending::Streamed` with why; gather it whole
(`Outgoing::gather`) only where the protocol needs all of it first — a
datagram, a frame, a record with its digest, a signature over the whole —
and declare `Sending::Gathered` with why: the runtime's byte budget for
sends counts the whole Message for a send that gathers. Each read opens the
body from its start, so a kept session that fails on reuse and carries the
exchange again on a new one reads it again, never from where the first
attempt left it. Where the protocol has a native identifier its far end
deduplicates by — a message id, a record key, an idempotency header — put
the key there and say so in the README; every other sends at least once.

And `Configured` (`.src/configured.rs`) beside it: the settings the
technology takes beyond the Location's address, declared once, and the one
constructor that takes what that declaration read (ADR-0064, amendment
2026-09-26):

```rust
impl Configured for KafkaTransport {
    const SETTINGS: &'static Settings = &Settings {
        technology: env!("CARGO_PKG_NAME"),
        settings: &[Setting {
            name: "topic",
            kind: Kind::Text,
            presence: Presence::Required,
            meaning: "The topic a Receive Location reads and a Send Location writes.",
            applies: Applies::Both,
        }],
    };

    fn configured(address: &str, settings: &Read) -> Result<Self> {
        Ok(Self::new(address, settings.text("topic")))
    }
}
```

The declaration is the technology's documentation, its form in VS Code and
the desktop editor, and what `configure` holds a Location's `settings` table
to at start; `open` reads a Location through it and calls `configured`. So:
never parse a setting by hand, write a default once — a constant the code
already holds is `Fixed::Duration(TIMEOUT)` or `Fixed::Integer(…)`, never
the value again — and declare a setting on the side that reads it. A secret
is never a setting: the Location's `credentials` names it. A technology that
takes nothing beyond its address declares `Settings::none(env!("CARGO_PKG_NAME"))`.
Its test holds the declaration sound (`SETTINGS.problems()` is empty) and
builds from a Location through `open`.

The runtime then gives the transport its node, once, as it builds it:
`Configured::on_node` with the node's location, `NodeLocation` (the owner's
*Option A*, 2026-10-03). A technology that records at the far end who holds
what — a claimed file's name, a lease's owner — overrides it, keeps the
location, and returns what that name still holds there, since a node being
built is a node starting. Every other technology leaves the default, which
keeps nothing. Never take the node through `configured` or a constructor of
your own.

Never bring a protocol name into the transport *capability* — protocol code
lives only in its own technology repository. That is the rule that keeps the
base protocol-agnostic, and what two technologies both need goes up into this
crate, never sideways (ADR-0044).

## The crate

`Cargo.toml` names the package for the repository and depends on the
capability, tracking `main` (ADR-0005):

```toml
[package]
name = "xmip-core-transport-<name>"

[dependencies]
transport = { package = "xmip-core-transport", git = "…", branch = "main" }
xcore = { package = "xmip-core", git = "…", branch = "main" }  # the settings shape
```

Implement the trait in `src/lib.rs`. Keep `receive` honest: *nothing there is
not an error* — an absent source returns an empty vector. `cargo test` and
`cargo clippy --all-targets -- -D warnings` pass before the change lands.

Keep `send` to one exchange on a session opened once: a protocol that
connects, handshakes or logs in holds its sessions in a `transport::Pool`
field, keyed by the address a target names, and implements
`transport::Pooled` for its client — `pool::alive` on the socket where
nothing else says the far end hung up. A session that fails on reuse is
replaced and the exchange goes again, so a send is at least once. The far
end that serves a send takes the exchange and does not wait for a goodbye:
the near end keeps its session. A user and a password are the capability's
`Login`, never a type of the technology's own. Only a protocol that frames
a Stream by the connection itself — raw TCP, closed to end it — connects
per message, and says why.

## Reaching a node

A transport reaches a node by being linked into `xmip-service`, never by
being installed beside it: the runtime's Module loader drives the contract
table alone, and the service is built without it
([built, not in the assembled service](../../../../../doc/architecture/estate-map.md#module-loading)). So a new
transport needs three more edits and a rebuild — a `transport-<name>`
feature in the estate root's `Cargo.toml`, its line in
`.src/service/built.rs`'s `transports()`, and the domain profile under
`deploy/profile/domain` that serves it — and then `Build-XmipService -Site
<name>` for each site that should carry it. Only then may a node's TOML name
it; a Location naming a transport its program was not built with is refused
as the node starts.

## Prove it

A transport is its own far end (ADR-0051): the technology ships the
capability's `Loopback`, its payload ceiling and its refusals, and the
Playground drives it through one adapter over that, by every content contract
at once, timed, sized and fault-injected. A new transport is its own loopback
and a line in the list, not a new scenario — ADR-0028 and
`test/core/playground/README.md`.

The far end is the capability's, never a struct of the technology's own: a
TCP protocol stands up `listening::Listening` over its `Accepting` — the
transport type, or a closure over the session it serves — a datagram protocol
`bound::Bound` over its `Reading`, and anything held in this process — a
device on a simulated bus, a radio's server, a pipe — `held::Held` with an
address and the one take. A protocol whose two ends meet in process keeps
its sessions in `standing::Standing`. `loopback::both_ends` and
`loopback::poke` are the two-thread dance and the bounded poke for a far
end that delivers onward, and `ceiling::within` is the one refusal of a
payload over the ceiling. What stays in the technology is its protocol: what
the far end does with the exchange once it has it (ADR-0051, amendment
2026-09-24).

Do not override `round` or `unblock` to say what the trait already knows
(ADR-0051, amendment 2026-09-25). A protocol whose far end answers as the
near end sends — a bus, a line with one master, a radio held in process, a
directory — declares `exchanges_in_order` and the round goes in order on one
thread. A datagram far end is `bound::Bound`, which says so, and is left to
its own timeout. The default `unblock` pokes a TCP listener; override it only
where a far end is released some other way — a path to connect to, a frame
that ends a transfer.
