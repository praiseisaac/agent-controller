# agent-controller documentation

Pluggable UI automation for agents: one `Controller` interface, five backends,
selected at runtime.

## Start here

- [architecture.md](architecture.md) — the `Controller` trait, factory, locators,
  per-backend persistence models, CLI surface.
- [session-management.md](session-management.md) — the `.agent-controller/`
  session store (per-item sessions, artifacts, resume).
- [adding-a-controller.md](adding-a-controller.md) — **write your own backend**
  (step-by-step + skeleton).

## Controllers

| Doc | Backend | Transport |
|---|---|---|
| [controllers/mac.md](controllers/mac.md) | `mac` | Accessibility API + CGEvent + System Events |
| [controllers/ios-sim.md](controllers/ios-sim.md) | `ios-sim` | `idb_companion` gRPC + `xcrun simctl` |
| [controllers/firefox.md](controllers/firefox.md) | `firefox` | WebDriver BiDi (per-instance daemon) |
| [controllers/chrome.md](controllers/chrome.md) | `chrome` | DevTools Protocol (CDP) |
| [controllers/safari.md](controllers/safari.md) | `safari` | `safaridriver` (W3C WebDriver/HTTP) |
| [controllers/windows.md](controllers/windows.md) | `windows` | UI Automation + SendInput (Windows-only) |
| [controllers/android-emu.md](controllers/android-emu.md) | `android-emu` | `adb` / `uiautomator` (cross-platform) |

## Install

See **[install.md](install.md)** for the one-liner, `install.sh`, and the full
build-from-source guide (prerequisites, `cargo install`, manual copy, updating,
uninstall, troubleshooting).

```sh
# one-liner (clone + build + install to ~/bin):
curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash
```

Then:

```sh
agent-controller doctor       # check/grant per-backend permissions
```

## MCP

`agent-controller mcp` exposes the backends as MCP tools (`navigate`, `snapshot`,
`click`, `type`, `press`, `scroll`, `screenshot`). Register with Claude Code:

```sh
claude mcp add agent-controller -- agent-controller mcp
```
