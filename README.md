# agent-controller

Pluggable UI automation for agents. A backend-agnostic core defines a single
`Controller` trait (snapshot → click → type → screenshot); concrete backends
implement it and are selected at runtime by a factory. The same CLI, session
store, `doctor`, and MCP server work across every backend.

| Backend | Target | Transport |
|---|---|---|
| `mac` | any native macOS app | Accessibility API + CGEvent + System Events |
| `windows` | any Windows app | UI Automation + SendInput *(Windows-only)* |
| `ios-sim` | a booted iOS Simulator | `idb_companion` gRPC + `xcrun simctl` |
| `android-emu` | an Android emulator/device | `adb` / `uiautomator` *(any host)* |
| `firefox` | Firefox | WebDriver BiDi (per-instance daemon) |
| `chrome` | Chrome | DevTools Protocol (CDP) |
| `safari` | Safari | `safaridriver` (W3C WebDriver/HTTP) |

Backends are conditionally compiled: macOS builds include mac/ios-sim/safari;
Windows builds include windows; firefox/chrome/android-emu are cross-platform.

## Install

One-liner (clones, builds, installs to `~/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash
# also brew-install missing deps (protoc, idb-companion):
curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash -s -- --install-deps
```

From a checkout:

```sh
./install.sh                  # build (release) + install to ~/bin
PREFIX=/usr/local/bin ./install.sh
agent-controller doctor       # check/grant per-backend permissions
```

### Windows

The `install.sh` one-liner is macOS/Unix-only; on Windows, build from a checkout.
You need **Rust with the MSVC toolchain** (`x86_64-pc-windows-msvc`, the rustup
default) — `protoc` is *not* required, since the only backend that needs it
(`ios-sim`) is macOS-only and skipped on Windows.

```powershell
git clone https://github.com/praiseisaac/agent-controller.git
cd agent-controller
cargo build --release          # builds windows + firefox + chrome + android-emu
.\target\release\agent-controller.exe --backend windows snapshot
```

Optionally copy `target\release\agent-controller.exe` somewhere on your `PATH`.
UI Automation needs no special permission, but a non-elevated process can't drive
**elevated** windows — run elevated to automate elevated apps. Details +
troubleshooting: **[docs/controllers/windows.md](docs/controllers/windows.md)**.

Build from source (`cargo install`, manual build, prerequisites, updating,
uninstall): **[docs/install.md](docs/install.md)**. The macOS build requires Rust
(rustup) and `protoc` (for the ios-sim gRPC codegen).

## Documentation

- **[docs/](docs/README.md)** — index
- [docs/architecture.md](docs/architecture.md) — trait, factory, locators, sessions
- Per-controller docs: [mac](docs/controllers/mac.md) ·
  [ios-sim](docs/controllers/ios-sim.md) · [firefox](docs/controllers/firefox.md) ·
  [chrome](docs/controllers/chrome.md) · [safari](docs/controllers/safari.md)
- **[Write your own backend](docs/adding-a-controller.md)** ·
  [Contributing](CONTRIBUTING.md)

## Workspace layout

```
crates/core      agent-controller-core    — Controller trait + types + factory iface + session store
crates/mac       agent-controller-mac     — macOS desktop (Accessibility API)
crates/ios-sim   agent-controller-ios-sim — iOS Simulator (idb_companion gRPC)
crates/firefox   agent-controller-firefox — Firefox (WebDriver BiDi + daemon)
crates/chrome    agent-controller-chrome  — Chrome (CDP)
crates/safari    agent-controller-safari  — Safari (safaridriver/WebDriver)
app              agent-controller         — CLI + factory registry + MCP server
```

## iOS Simulator backend

Drives a booted simulator by talking **gRPC directly to `idb_companion`** (no
Python `idb` client — it's broken on Python 3.14). App launch / URL open / boot
go through `xcrun simctl`; the accessibility tree, input (HID), and screenshots
go through idb's gRPC service.

- `snapshot` — `accessibility_info` → an `@ref`-tagged element tree (cached to
  the temp dir so `click @e6` works across separate CLI invocations).
- `click` / `type` / `press` / `scroll` — synthesized via idb's `hid` stream
  (touch / keyboard usage codes / swipe).
- `screenshot` — idb's `screenshot` RPC → PNG.

Verified on **macOS 26.4 / Xcode 26.5 / iOS 26.5** (iPhone 17 Pro).

### Prerequisites

```sh
brew install facebook/fb/idb-companion   # the gRPC companion (bottle works on Xcode 26)
# Xcode + a booted simulator (xcrun simctl). No Python idb client needed.
```

### Build

```sh
cargo build --release
```

(Requires `protoc` for the gRPC codegen: `brew install protobuf`.)

### Usage

```sh
agent-controller status                       # bound sim + capabilities
agent-controller use com.apple.Preferences    # launch an app (bundle id)
agent-controller use https://apple.com        # open a URL
agent-controller use home                      # go to the home screen
agent-controller snapshot                      # accessibility tree with @refs
agent-controller click @e6                     # tap by ref
agent-controller click "General"               # tap by label
agent-controller click "(201,406)"             # tap by point
agent-controller type "hello"                  # type into the focused field
agent-controller press home                    # Home / Lock / Siri / Enter / ...
agent-controller scroll down 500               # swipe to scroll
agent-controller screenshot out.png            # capture PNG

# global flags: --udid <UDID>  --json  --backend <name>
```

`--udid` selects among multiple booted sims (defaults to the only booted one).
`--json` emits structured output for agents.

## MCP server

`agent-controller mcp` runs a stdio MCP (JSON-RPC) server exposing the backends
as tools, so an MCP client (e.g. Claude) can drive any backend. Tools: `navigate`,
`snapshot`, `click`, `type`, `press`, `scroll`, `screenshot` — each takes a
`backend` arg (`mac`/`ios-sim`/`firefox`/`chrome`/`safari`) plus optional
`session`/`udid`/`app`. `screenshot` returns an image; the rest return text.

Register with Claude Code:

```sh
claude mcp add agent-controller -- agent-controller mcp
```

Then ask Claude to, e.g., "snapshot the ios-sim and tap Settings", or "open
example.com in chrome and screenshot it".

## Safari backend

Drives Safari via `safaridriver` (W3C WebDriver over HTTP). safaridriver is the
persistent server; we keep it running and reuse a stored `sessionId` across CLI
invocations. Instances are keyed by `--session <name>` → its own port + session.

**One-time setup:** enable Remote Automation —
`safaridriver --enable` (authenticate), or Safari ▸ Settings ▸ Advanced ▸ "Show
features for web developers", then Develop ▸ **Allow Remote Automation**.

```sh
agent-controller --backend safari use example.com
agent-controller --backend safari snapshot
agent-controller --backend safari click @e2
```

## Chrome backend

Drives Chrome via the DevTools Protocol (CDP). Unlike Firefox's BiDi, CDP allows
transient connections, so each CLI invocation reconnects to a persistent Chrome —
**no daemon needed**. Instances are keyed by `--session <name>` (default
`default`) → an isolated profile + debug port.

```sh
agent-controller --backend chrome use example.com    # launches Chrome + navigates
agent-controller --backend chrome snapshot            # @ref tree
agent-controller --backend chrome click @e2           # click by ref
agent-controller --backend chrome type "hello"        # Input.insertText
agent-controller --backend chrome press "Cmd+A"       # Input.dispatchKeyEvent
```

> Installing the binary: use `rm ~/bin/agent-controller && cp target/release/agent-controller ~/bin/`
> (not a plain `cp` overwrite). Overwriting the file in place while a long-lived
> process — e.g. the firefox daemon, which is `current_exe` — has it mapped
> invalidates its code signature and macOS then kills it with "Killed: 9".

## Firefox backend

Drives Firefox via WebDriver BiDi (vendored from agent-browser-firefox). A BiDi
session is bound to its WebSocket connection, so a long-lived **daemon** owns the
connection for the browser's lifetime; CLI invocations are thin clients over a
Unix socket. Instances are keyed by `--session <name>` (default `default`) → an
isolated profile + debug port + daemon, so multiple browsers coexist and persist
across invocations.

```sh
agent-controller --backend firefox use example.com   # launches Firefox + navigates
agent-controller --backend firefox snapshot           # @ref tree (data-abf-ref tags)
agent-controller --backend firefox click @e2          # click by ref
agent-controller --backend firefox click "Learn more" # click by text
agent-controller --backend firefox click "css:#submit" # click by CSS selector
agent-controller --backend firefox --session work use github.com  # second instance
```

## macOS backend

Controls any native app via the Accessibility API (AX-tree-first). **Control is
foreground-only**: the target app is activated (brought to the front) before
acting — the same way you can't use a browser window while it's being driven. The
app is "owned" during automation.

- `snapshot` reads the AX tree → `@ref`s (same format as every backend).
- `click "Label"` / `click role:AXButton` act via **`AXPress`**; `click @ref` /
  `click (x,y)` use a synthetic `CGEvent` mouse click.
- `type` and `press Cmd+B` go through **System Events** (`osascript`), which
  reliably delivers keystrokes to the frontmost app and respects typing
  attributes (so bold/italic formatting applies). `menu "Format>Font>Bold"`
  invokes native menus via `AXPress`.
- `screenshot` captures the app's window by id (occlusion-proof).

```sh
agent-controller --backend mac --app TextEdit type "hello world"
agent-controller --backend mac --app TextEdit press "Cmd+A"
agent-controller --backend mac --app TextEdit press "Cmd+B"   # bold the selection
agent-controller --backend mac --app Finder snapshot
agent-controller displays            # screen bounds + backing scale
agent-controller doctor              # check/grant permissions
```

### Coordinate model (multi-screen / Retina)

AX frames, `CGEvent` input, and `screencapture` all share **one global point
space** — top-left origin of the main display, spanning every monitor (secondary
displays get offset/negative coordinates by arrangement). We never use AppKit's
bottom-left `NSScreen` space. So **element-based interaction (`@ref`/label/role)
is screen-agnostic** with no per-display math.

The one caveat: **screenshots are device pixels** (Retina = 2×, and scale can
differ per display in mixed-DPI setups) while clicks take **points**. To map a
pixel read off a screenshot back to a click point, divide by that display's
`scale` from `agent-controller displays`.

### Permissions

- **Accessibility** — required for AX read + input. Grant it to your *terminal
  app* (e.g. iTerm/Terminal) in System Settings ▸ Privacy & Security ▸
  Accessibility. Without it, AX commands error with a clear message.
- **Screen Recording** — required for `screenshot`.

## License

Licensed under either of

- MIT license ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
