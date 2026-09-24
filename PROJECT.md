# agent-controller — project status & plan

> Context-retention summary. For deep docs see `docs/` (architecture, install,
> session-management, adding-a-controller, per-controller pages).

## Goal

One tool that drives many UI-automation targets — native macOS apps, the iOS
Simulator, an Android emulator, a Windows desktop, and the Firefox/Chrome/Safari
browsers — behind a **single `Controller` interface**, selected at runtime by a
factory. The CLI, session store, `doctor`, and MCP server are written once
against `dyn Controller`; adding a backend never touches them.

Origin: extend the existing `agent-browser-firefox` idea to the whole desktop,
abstracted so every target inherits a common interface.

## Architecture

- **Cargo workspace.** `crates/core` defines the `Controller` trait, shared types
  (`Locator`, `Snapshot`, `Element`, `Rect`, `Image`, `Capabilities`), the
  `Backend` enum, the `BackendFactory` trait, and the on-disk `SessionStore`.
  Each backend is its own crate. `app/` is the `agent-controller` binary (CLI +
  factory registry + MCP server).
- **Controller verbs:** `navigate, snapshot, click, type_text, press, scroll,
  screenshot, menu(default-unsupported), backend, target, capabilities`.
- **Locators:** `@ref` (from snapshot), `Label`/`Text`, `Role`, `Css` (browsers,
  `css:` prefix), `Point` (`(x,y)`). Every backend renders the same outline:
  `[@e3] role "label" (x,y WxH)`.
- **Factory:** `app/src/factory.rs::registry()` maps `Backend` → `BackendFactory`;
  `create()` derives a session id, loads/creates the record, opens (resume or
  spawn), persists. One match arm per backend.
- **Sessions:** `~/.agent-controller/sessions/<backend>/<instance>/` (or a
  project-local `.agent-controller/`, or `$AGENT_CONTROLLER_HOME`). Holds
  `session.json` (config + live-process `runtime`), `refs.json`, `screenshots/`,
  logs. Keyed by the item (udid, bundle id, profile name, serial).
- **Cross-platform compilation:** OS-specific crates are `#![cfg(target_os=…)]`
  with platform deps under `[target.'cfg(target_os=…)']` → compile empty
  off-platform; `ios-sim` `build.rs` skips `protoc` off macOS; `app` deps +
  registry arms are `cfg`-gated. macOS build = mac/ios-sim/safari + firefox/
  chrome/android-emu; Windows build = windows + firefox/chrome/android-emu.

## Backends

| Backend | Target | Transport | Persistence | Status |
|---|---|---|---|---|
| `mac` | macOS apps | Accessibility API + System Events (keys) + CGEvent (mouse) | foreground, stateless | ✅ verified |
| `ios-sim` | iOS Simulator | `idb_companion` gRPC (tonic) + `xcrun simctl` | companion server, reconnect | ✅ verified |
| `firefox` | Firefox | WebDriver BiDi (ws) | our daemon owns ws; CLI = localhost-TCP client | ✅ verified |
| `chrome` | Chrome | DevTools Protocol (CDP, ws) | stateless reconnect (no daemon) | ✅ verified |
| `safari` | Safari | safaridriver (W3C WebDriver/HTTP) | reuse stored sessionId | ✅ verified |
| `android-emu` | Android emulator/device | `adb` + `uiautomator` (cross-platform) | stateless | ⚠️ builds + dispatches; needs adb+device to run |
| `windows` | Windows apps | UI Automation + SendInput + GDI (Windows-only) | foreground | ⚠️ written; build/iterate on Windows |

## Key decisions & hard-won learnings

- **idb works on Xcode 26 / iOS 26** despite a 2022 release — use the companion's
  gRPC directly (the Python `idb` CLI is broken on Python 3.14).
- **macOS AX can't see inside the iOS Simulator** → a dedicated `ios-sim` backend.
- **mac is foreground-only** (user's call): activate the app + wait until it's
  actually frontmost, then drive via **System Events keystrokes** — raw CGEvent
  keyboard is unreliable on current macOS; macOS routes formatting/shortcuts to
  the key window's first responder.
- **A BiDi session is bound to its WebSocket** → Firefox needs a long-lived
  daemon; a fresh connection can't reclaim a session. CDP allows transient
  connections → Chrome needs no daemon.
- **Firefox daemon IPC is localhost TCP** (was Unix socket) so it compiles/runs on
  Windows too. Chrome process-detach is `cfg`-gated (unix `process_group` vs
  windows `creation_flags`). Browser binary paths are per-OS.
- **Reinstall the binary with `rm`+`cp` (or `install.sh`), never in-place `cp`** —
  the firefox daemon runs as `current_exe`, and overwriting a mapped binary makes
  macOS kill it with "Killed: 9".
- **Multi-screen/Retina:** AX + CGEvent + screencapture share one global point
  space; only screenshot pixels need the per-display scale (`displays` command).

## Tooling shipped

- **CLI:** `use/snapshot/click/type/press/scroll/screenshot/menu/status` +
  `sessions`, `session show|rm|path|prune`, `config show|init|path`, `displays`,
  `doctor`, `mcp`.
- **Browser launch config:** `core::config::LaunchConfig` (width/height/x/y/
  headless/args), layered `<home>/config.toml` (`[launch]`, `[launch.<backend>]`)
  → env → `--window-size`/`--window-position`/`--headless` (MCP `window_size`/
  `headless`). Default 1360x800 (~1.7:1); one dimension derives the other at that
  ratio. Resolved once in `factory::create` into `Options.launch`; applied on
  cold start only (chrome: `--window-size` + CDP `setWindowBounds`; firefox:
  `--width/--height` via the daemon's `--launch` arg; safari: WebDriver
  `window/rect`). `session.json` `config.launch` records what a browser started with.
- **MCP server:** `agent-controller mcp` (stdio) exposes the 7 verbs as tools
  (each takes a `backend` arg). Registered with Claude Code at user scope
  (`claude mcp add agent-controller -- agent-controller mcp`). No re-registration
  needed when backends change; just reinstall the binary and restart the session.
- **Install:** `install.sh` — remote one-liner (`curl … | bash`, clones+builds)
  and from-checkout; `--install-deps`, `--debug`, `PREFIX`. Needs Rust + `protoc`
  (macOS only). See `docs/install.md`.
- **Docs:** `docs/` (architecture, install, session-management, adding-a-controller,
  per-controller). `CONTRIBUTING.md`. **License: MIT OR Apache-2.0.**

## Verified vs pending

- Verified live on macOS: mac (incl. bold-in-TextEdit), ios-sim (Settings nav),
  firefox + chrome (example.com → IANA; **both signed into ListedKit via email
  OTP fetched through email-mcp**), safari (example.com nav), displays, doctor,
  MCP round-trip.
- Pending (need the right machine/tool): android-emu runtime (adb + AVD); the
  **windows** backend's compile/run (build on Windows, iterate on `windows`-crate
  API mismatches).

## Repo / next steps

- Git: remote `git@github.com:praiseisaac/agent-controller.git`, branch `main`,
  **nothing committed/pushed yet**.
- Next:
  1. **Initial commit + push** (needed for the install one-liner URL to resolve;
     repo must be **public** for unauthenticated `curl|bash`).
  2. Build the **windows** backend on Windows; fix `windows`-crate API nits.
  3. Install Android platform-tools + an AVD; verify **android-emu**.
  4. Optional: add android/windows prereq checks to `doctor`; window-by-title
     targeting + `ValuePattern.SetValue` for windows; expose `eval`/network as
     verbs for browsers.
