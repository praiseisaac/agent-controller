# Adding a controller (backend)

A backend teaches `agent-controller` to drive a new target (an app, a device, a
browser, an emulator…). You implement two traits in a new crate and register it
in one place — the CLI, session store, `doctor`, and MCP server then work for
your backend automatically, because they only ever talk to `dyn Controller`.

This guide uses a fictional `widget` backend as the running example.

## What you implement

1. **`Controller`** — the verbs (`navigate`, `snapshot`, `click`, `type_text`,
   `press`, `scroll`, `screenshot`, optional `menu`, plus `backend`/`target`/
   `capabilities`).
2. **`BackendFactory`** — `identify(opts)` (derive a stable session id without
   spawning) and `open(rec, store, opts)` (resume the live target or start it,
   return a `Box<dyn Controller>`).

Both traits live in `agent-controller-core` (`crates/core/src/lib.rs`); read it
once — it's small.

## Step 1 — create the crate

```
crates/widget/
├── Cargo.toml
└── src/lib.rs
```

```toml
# crates/widget/Cargo.toml
[package]
name = "agent-controller-widget"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
name = "agent_controller_widget"
path = "src/lib.rs"

[dependencies]
agent-controller-core = { path = "../core" }
anyhow.workspace = true
async-trait.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
# + whatever transport you need (reqwest, tokio-tungstenite, an FFI crate, …)
```

## Step 2 — add the `Backend` enum variant (core)

`Backend` is a closed enum, so add your variant in `crates/core/src/lib.rs`:

- add `Widget` to `enum Backend`
- add an arm to `Backend::as_str` (`Backend::Widget => "widget"`)
- add a match arm to `FromStr` (`"widget" => Backend::Widget`)

This is the only edit to `core`.

## Step 3 — implement the controller + factory

```rust
// crates/widget/src/lib.rs
use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image,
    Locator, Options, Rect, Result, ScrollDir, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;

pub struct WidgetController {
    target: String,
    // transport handle(s): an HTTP client, a socket, an FFI session, …
}

#[async_trait]
impl Controller for WidgetController {
    async fn navigate(&self, target: &str) -> Result<()> { todo!() }

    async fn snapshot(&self) -> Result<Snapshot> {
        // Produce Elements with stable @refs ("e1","e2",…). Reuse the same
        // outline everyone else uses: role, label, value, frame, actions.
        Ok(Snapshot { elements: vec![] })
    }

    async fn click(&self, loc: &Locator) -> Result<()> {
        match loc {
            Locator::Ref(r)            => { /* resolve @r */ todo!() }
            Locator::Label(s) | Locator::Text(s) => { todo!() }
            Locator::Role { role, name } => { todo!() }
            Locator::Point { x, y }    => { todo!() }
            Locator::Css(_)            => Err(anyhow!("CSS not supported by widget")),
        }
    }

    async fn type_text(&self, text: &str) -> Result<()> { todo!() }
    async fn press(&self, key: &str) -> Result<()> { todo!() }
    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> { todo!() }
    async fn screenshot(&self) -> Result<Image> {
        Ok(Image { data: vec![], format: "png".into() })
    }

    // menu() has a default that returns "unsupported"; override only if relevant.

    fn backend(&self) -> Backend { Backend::Widget }
    fn target(&self) -> String { self.target.clone() }
    fn capabilities(&self) -> Capabilities {
        Capabilities { eval: false, network: false, menus: false, coordinates: true, screenshot: true }
    }
}

pub struct WidgetFactory;

#[async_trait]
impl BackendFactory for WidgetFactory {
    fn backend(&self) -> Backend { Backend::Widget }

    /// Derive the session id WITHOUT spawning anything (e.g. resolve a device id).
    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let name = opts.session.clone().unwrap_or_else(|| "default".into());
        Ok(Identity { id: format!("widget/{name}"), target: name })
    }

    /// Resume the live target if reachable (see `rec.runtime`), else start it.
    /// Record how to re-attach into `rec.runtime` / `rec.config`.
    async fn open(&self, rec: &mut SessionRecord, store: &SessionStore, opts: &Options)
        -> Result<Box<dyn Controller>>
    {
        // let paths = store.paths(&rec.id)?;       // refs.json / screenshots/ / log
        // rec.runtime.endpoint = Some(...);        // socket/url/port to reconnect
        // rec.runtime.pids = vec![pid];
        // rec.runtime.alive = true;
        // rec.config = serde_json::json!({ ... });  // backend-specific, opaque to core
        Ok(Box::new(WidgetController { target: rec.target.clone() }))
    }
}
```

## Step 4 — register it (3 lines, one file)

In `app/src/factory.rs`:

```rust
use agent_controller_widget::WidgetFactory;

pub fn registry(backend: Backend) -> Result<Box<dyn BackendFactory>> {
    Ok(match backend {
        // …existing arms…
        Backend::Widget => Box::new(WidgetFactory),
    })
}
```

And add the crate to the workspace `members` (root `Cargo.toml`) and as a
dependency of `app` (`app/Cargo.toml`). That's it — no CLI/MCP changes.

## The value types you'll use

- **`Locator`** — `Ref` / `Label` / `Text` / `Role` / `Css` / `Point`. Support
  what makes sense; return an error for the rest and reflect it in
  `capabilities()`.
- **`Snapshot { elements: Vec<Element> }`** and **`Element { ref, role, label,
  value, frame, enabled, actions }`** — `Snapshot::render()` produces the shared
  `[@e1] role "label" (x,y WxH)` outline. Number refs `e1`, `e2`, … in document/
  tree order so they're stable within a snapshot.
- **`Rect`** — logical points, top-left origin. `center()` helps coordinate
  clicks.
- **`Image { data, format }`** — raw bytes (usually PNG).
- **`Capabilities`** — advertise `eval`/`network`/`menus`/`coordinates`/
  `screenshot` honestly; callers gate optional behavior on these.

## Persistence: pick the model that fits your transport

The hard part is keeping the target alive between ephemeral CLI runs. Use an
existing backend as a template:

| If your transport… | Model | Template |
|---|---|---|
| is queried fresh each time (the app persists itself) | **stateless** | `crates/mac` |
| has a long-lived local server you reconnect to | **reconnect** | `crates/ios-sim` (gRPC), `crates/chrome` (CDP) |
| binds a session to one connection | **your own daemon** | `crates/firefox` |
| is request/response with a server-side session id | **store the session id** | `crates/safari` |

Record re-attach info in `rec.runtime` (`endpoint`, `pids`, `alive`) and any
backend-specific bits in `rec.config` (opaque JSON). Use
`store.paths(&rec.id)` for `refs.json`, `screenshots/`, and a log file.

## The `@ref` convention

`snapshot` assigns `@eN` ids; `click @eN` resolves them. Two patterns:
- **DOM-backed** (browsers): tag nodes with a `data-abf-ref` attribute in the
  page so refs survive in the live DOM across invocations — no local cache.
- **Cached** (mac/ios-sim): write the ref→element map to `store.paths(id).refs`
  and reload it to resolve `@eN`.

## Verify

```sh
cargo build
agent-controller --backend widget status
agent-controller --backend widget snapshot
agent-controller --backend widget click @e1
# MCP: it's already a tool — no extra work
echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"snapshot","arguments":{"backend":"widget"}}}' \
  | agent-controller mcp
```

If your backend needs a permission or external tool, add a check to
`agent-controller doctor` (see `app/src/main.rs::doctor` and
`crates/mac/src/setup.rs` / `crates/safari` for examples).

## Conventions / checklist

- [ ] `cargo build` is warning-free.
- [ ] `snapshot` uses the shared outline + stable `@eN` refs.
- [ ] Unsupported locators/verbs return a clear error; `capabilities()` matches.
- [ ] `open` records enough in `rec.runtime`/`rec.config` to re-attach.
- [ ] Long-lived child processes are detached (own process group) and not killed
      when the CLI exits.
- [ ] A line in `docs/controllers/<widget>.md` + a row in `docs/README.md` and the
      top-level README backend table.
