# Architecture

`agent-controller` is one tool that drives many automation targets — native
macOS apps, the iOS Simulator, and the Firefox/Chrome/Safari browsers — behind a
single interface. A backend is chosen at runtime; everything above the interface
(CLI, session store, MCP server, `doctor`, `displays`) is written once.

```
CLI  /  MCP server  (agent-controller mcp)
                │  builds via factory::create → Box<dyn Controller>
                ▼
        ┌───────────────── Controller (trait) ─────────────────┐
        │ navigate · snapshot · click · type · press · scroll   │
        │ screenshot · menu · backend · target · capabilities   │
        └───────────────────────────────────────────────────────┘
            │          │           │          │           │
          mac       ios-sim     firefox     chrome      safari
        AX + CGEvent  idb gRPC   BiDi/daemon  CDP        safaridriver
        + SysEvents   + simctl   (WebSocket)  (WebSocket) (WebDriver/HTTP)
```

## The crates

| Crate | Responsibility |
|---|---|
| `agent-controller-core` | The `Controller` trait, shared value types (`Locator`, `Snapshot`, `Element`, `Rect`, `Image`, `Capabilities`), the `Backend` enum, the `BackendFactory` trait, and the on-disk `SessionStore`. Knows no concrete backend. |
| `agent-controller-mac` | macOS desktop backend (Accessibility API + CGEvent + System Events + ScreenCaptureKit/`screencapture`). |
| `agent-controller-ios-sim` | iOS Simulator backend over `idb_companion` gRPC + `xcrun simctl`. |
| `agent-controller-firefox` | Firefox backend over WebDriver BiDi, with a per-instance daemon that owns the WebSocket. |
| `agent-controller-chrome` | Chrome backend over the DevTools Protocol (CDP). |
| `agent-controller-safari` | Safari backend over `safaridriver` (W3C WebDriver/HTTP). |
| `agent-controller-windows` | Windows desktop backend (UI Automation + SendInput + GDI). Windows-only. |
| `agent-controller-android-emu` | Android emulator/device backend over `adb`/`uiautomator`. Cross-platform. |
| `agent-controller` (binary) | CLI + factory registry + MCP server; depends on each backend crate (OS-gated). |

### Cross-platform compilation

OS-specific crates are gated so the workspace builds on every host: each is
`#![cfg(target_os = "…")]` with its platform-only dependencies under
`[target.'cfg(target_os = "…")'.dependencies]`, so it compiles to an empty crate
off-platform (and `ios-sim`'s `build.rs` skips `protoc` off macOS). The `app`
depends on backend crates via target-specific dependencies, and `factory::registry`
gates each arm with `#[cfg(target_os = "…")]` — unavailable backends return a
clear "not available on this platform" error. macOS build = mac/ios-sim/safari +
firefox/chrome/android-emu; Windows build = windows + firefox/chrome/android-emu.

## The `Controller` trait

Every backend implements this (in `core/src/lib.rs`):

```rust
#[async_trait]
pub trait Controller: Send + Sync {
    async fn navigate(&self, target: &str) -> Result<()>;
    async fn snapshot(&self) -> Result<Snapshot>;
    async fn click(&self, loc: &Locator) -> Result<()>;
    async fn type_text(&self, text: &str) -> Result<()>;
    async fn press(&self, key: &str) -> Result<()>;
    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()>;
    async fn screenshot(&self) -> Result<Image>;
    async fn menu(&self, path: &str) -> Result<()>;     // default: unsupported
    fn backend(&self) -> Backend;
    fn target(&self) -> String;
    fn capabilities(&self) -> Capabilities;
}
```

## Locators

Backends interpret the variants they support; others return an "unsupported" error
(advertised via `capabilities()`):

| Locator | CLI syntax | mac | ios-sim | browsers |
|---|---|---|---|---|
| `Ref` | `@e3` | ✓ | ✓ | ✓ (`data-abf-ref`) |
| `Label` / `Text` | `"Sign in"` | ✓ (AX title/desc) | ✓ (AXLabel) | ✓ (text match) |
| `Role` | (programmatic) | ✓ | ✓ | ✓ |
| `Css` | `css:#email` | — | — | ✓ |
| `Point` | `(120,40)` | ✓ | ✓ | ✓ |

Every backend renders a `snapshot` to the **same** outline format:
`[@e3] <role> "<label>"  (x,y WxH)`, so agents read all targets identically.

## The factory + `BackendFactory`

`app/src/factory.rs` maps a `Backend` to its `BackendFactory`, then:

```
discover SessionStore
  → factory.identify(opts)   → session id ("<backend>/<instance>") + target
  → store.load(id) or new SessionRecord
  → factory.open(&mut rec, store, opts)   → resume live process or start one
  → store.save(rec)   → Box<dyn Controller>
```

Adding a backend is one new match arm in `registry()` plus its crate — nothing
else changes.

## Sessions & persistence

Sessions are keyed by the controlled item (`mac/<bundle>`, `ios-sim/<udid>`,
`chrome/<name>`, …) and stored under a `.agent-controller/` home (discovered via
`--home` / `AGENT_CONTROLLER_HOME` / a project-local `.agent-controller/` /
`~/.agent-controller/`). Each session dir holds `session.json` (config + live
process `runtime`), `refs.json` (last snapshot's `@ref`s, where applicable),
`screenshots/`, and logs. See [session-management.md](session-management.md).

## Persistence models per backend

The hard part of each backend is keeping the target alive between ephemeral CLI
invocations. Each transport needs something different:

| Backend | What persists | How a new invocation re-attaches |
|---|---|---|
| mac | the app itself | re-query the AX tree (stateless) |
| ios-sim | `idb_companion` server | reconnect gRPC on a recorded port |
| firefox | our **daemon** (owns the BiDi ws) | localhost-TCP client → daemon |
| chrome | the Chrome process | reconnect CDP (transient connections OK) |
| safari | `safaridriver` + its session | reuse the stored `sessionId` over HTTP |

## CLI surface

```
agent-controller [--backend B] [--session N] [--udid U] [--app A] [--home D] [--json] [--takeover]
                 [--window-size WxH] [--window-position X,Y] [--headless] <cmd>
  use <target> · snapshot · click <loc> · type <text> · press <key>
  scroll <dir> [amount] · screenshot [path] · menu <path> · status
  sessions · session show|rm|path|prune · config show|init|path · displays · doctor · mcp · version
```
