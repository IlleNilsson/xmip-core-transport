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
`test/playground/README.md`.
