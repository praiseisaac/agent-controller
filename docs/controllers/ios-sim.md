# ios-sim controller

Controls a booted **iOS Simulator** through `idb` and `xcrun simctl`.

- Crate: `crates/ios-sim` (`agent-controller-ios-sim`)
- Backend id: `ios-sim`
- Session id: `ios-sim/<udid>`

## Why a dedicated backend

The Simulator is a macOS app, but the **macOS Accessibility API does not see the
simulated iOS UI** — `AXUIElement` only exposes Simulator.app's own chrome. So the
iOS accessibility tree is reached through `idb` instead.

## Transport & implementation

`idb_companion` is a long-lived gRPC server (one per simulator). We talk to it
**directly over gRPC** (via `tonic` + a vendored, trimmed `idb.proto`) — the
Python `idb` CLI is not used (it's broken on Python 3.14). App lifecycle uses
`xcrun simctl`.

| Concern | Mechanism | File |
|---|---|---|
| gRPC client / companion lifecycle | spawn/reuse `idb_companion` on a per-udid port, `tonic` | `src/companion.rs`, `src/lib.rs` |
| Snapshot | `accessibility_info` (LEGACY) → `@ref` tree | `src/snapshot.rs` |
| Tap / type / press / scroll | `hid` event stream (touch, USB-HID key codes, swipe) | `src/lib.rs`, `src/keymap.rs` |
| Screenshot | gRPC `screenshot` → PNG | `src/lib.rs` |
| Launch / open url / boot | `xcrun simctl` | `src/lib.rs` |

Protocol RPCs used: `describe`, `accessibility_info`, `hid`, `screenshot`,
`open_url`, `launch`, `list_apps`, `terminate` (`crates/ios-sim/proto/idb.proto`).

## Prerequisites & setup

```sh
brew install facebook/fb/idb-companion     # the gRPC companion
pip3 install fb-idb                         # NOT required (we use gRPC directly)
```

- Xcode + a booted simulator (`xcrun simctl boot <udid>` / open Simulator.app).
- Only `idb_companion` is needed; the tool spawns/reuses it automatically.

Verified on macOS 26 / Xcode 26 / iOS 26 (the 2022 `idb_companion` bottle loads
the current CoreSimulator + AccessibilityPlatformTranslation frameworks fine).

## Session / persistence model

The session id is the UDID (`--udid`, else the single booted sim). A persistent
`idb_companion` runs on a deterministic per-udid port; each invocation reconnects
its gRPC channel. `refs.json` (snapshot `@ref`s) and `screenshots/` live in the
session dir.

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✗ | ✗ | ✗ | ✓ | ✓ |

## Commands & examples

```sh
agent-controller --backend ios-sim status                         # bound sim
agent-controller --backend ios-sim use com.apple.Preferences      # launch app (bundle id)
agent-controller --backend ios-sim use https://apple.com          # open a URL
agent-controller --backend ios-sim use home                       # home screen
agent-controller --backend ios-sim snapshot
agent-controller --backend ios-sim click @e6                      # tap by ref
agent-controller --backend ios-sim click "General"                # tap by label
agent-controller --backend ios-sim type "hello"
agent-controller --backend ios-sim press home                     # Home/Lock/Siri/Enter/...
agent-controller --backend ios-sim scroll down 500
agent-controller --backend ios-sim screenshot out.png             # default: session screenshots/
# multiple booted sims:
agent-controller --backend ios-sim --udid <UDID> snapshot
```

`navigate` cold-starts apps (terminate→launch) so they open to a predictable
state. Text entry sets the focused field; `type` uses HID keystrokes.

## Limitations & troubleshooting

- **`idb_companion` not found** → `brew install facebook/fb/idb-companion`.
- **"not booted"** → boot the sim (`xcrun simctl boot <udid>` or open Simulator).
- **Empty snapshot** → the sim is on a blank screen; launch an app first.
- Tapping near the very bottom can hit the home-indicator gesture zone; prefer
  `@ref`/label locators over raw points.
