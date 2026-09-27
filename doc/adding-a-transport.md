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

`Transport` (`.src/protocol.rs`) — five methods, and nothing in it names a
protocol:

```rust
pub trait Transport {
    fn name(&self) -> &'static str;                          // the token in the repo name
    fn directions(&self) -> Directions;                      // receive, send, or both
    fn receive(&self) -> Result<Vec<Arrived>>;               // empty vec = nothing arrived
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()>;
    fn claims(&self) -> Option<&dyn ResourceClaim> { None }  // exactly-once pickup (ADR-0024)
}
```

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
