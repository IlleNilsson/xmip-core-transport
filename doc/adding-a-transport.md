# Adding a transport

Moved here from the estate root on 2026-09-12 (ADR-0020 clause 3: the document lives where its subject lives).


### 1. The base you implement

`Transport` (`module/capability/transport/.src/protocol.rs`) — five methods,
and nothing in it names a protocol:

```rust
pub trait Transport {
    fn name(&self) -> &'static str;                          // the token in the repo name
    fn directions(&self) -> Directions;                      // receive, send, or both
    fn receive(&self) -> Result<Vec<Arrived>>;               // empty vec = nothing arrived
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()>;
    fn claims(&self) -> Option<&dyn ResourceClaim> { None }  // exactly-once pickup (ADR-0024)
}
```

Never bring a protocol name into the transport *capability* — protocol code lives
only in its own technology repository (that is the rule that keeps the base
protocol-agnostic).

### 2. Create the repository and module

The repository is `xmip-core-transport-<name>`; it mounts as a submodule at
`<name>` **directly inside the transport capability's repository** (ADR-0016,
amended 2026-09-07), and it **depends on the capability, never the reverse**
(`repository-model.md`).

1. **Declare it** in `architecture.toml` under `[xmip.core.transport.<name>]`,
   with a `maturity` (`reserved` → `planned` → `scaffolded` → `supported`).
2. **Create the GitHub repo.** `gh repo create xmip-core-transport-<name> --public`
   — run this yourself; the assistant is blocked from creating repositories.
3. **Scaffold** from the working template — copy the layout of
   `module/capability/contract/csv/` (Cargo.toml, `src/lib.rs`,
   README, `rust-toolchain.toml`, LICENSE, tests). In `Cargo.toml`:
   ```toml
   [package]
   name = "xmip-core-transport-<name>"

   [dependencies]
   transport = { package = "xmip-core-transport", git = "…", branch = "main" }
   ```
4. **Implement** the trait in `src/lib.rs`. Keep `receive` honest: *nothing there
   is not an error* — an absent source returns an empty vector.
5. **Green it:** `cargo test` and `cargo clippy --all-targets -- -D warnings`.

### 3. Mount and land

```bash
# inside the transport capability repo
git -C module/capability/transport submodule add \
    https://github.com/<you>/xmip-core-transport-<name> <name>

# from the estate root
Import-Module ./Xmip/Xmip.psd1 -Force
Publish-XmipChange -Message 'Add the <name> transport'
```

`Publish-XmipChange` (alias `xgit`) is nesting-aware: it tests and lands the technology,
records the gitlink in the capability, then pins the superproject — in that order.

### 4. Prove it

Add the transport to the **Playground**'s pingpong scenario (ADR-0028): one new
`RoundTrip` adapter, and it is exercised by every content contract at once,
timed, sized and fault-injected — not a new test, a new adapter.

---

