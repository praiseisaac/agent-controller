# chrome controller

Controls Chrome over the **DevTools Protocol (CDP)**.

- Crate: `crates/chrome` (`agent-controller-chrome`)
- Backend id: `chrome`
- Session id: `chrome/<instance-name>` (`--session`, default `default`)

## No daemon needed

Unlike Firefox's BiDi, **CDP permits transient connections** and doesn't bind an
exclusive session to one socket. So each CLI invocation simply reconnects to the
persistent Chrome process — no daemon. This is the simplest browser backend.

## Transport & implementation

| Concern | Mechanism | File |
|---|---|---|
| CDP WebSocket client | multiplexed `{id,method,params}` → `{id,result}` | `src/cdp.rs` |
| Launch + target discovery | `--remote-debugging-port`, `GET /json` for the page ws | `src/launch.rs` |
| Navigate | `Page.navigate` + poll `document.readyState` | `src/lib.rs` |
| Snapshot / click | `Runtime.evaluate` (JS tagging `data-abf-ref`) | `src/lib.rs` |
| Type | `Input.insertText` (realistic: `Input.dispatchKeyEvent` per key) | `src/lib.rs` |
| Press | `Input.dispatchKeyEvent` (modifier bitmask + key codes) | `src/lib.rs` |
| Scroll | `Runtime.evaluate` `window.scrollBy` | `src/lib.rs` |
| Screenshot | `Page.captureScreenshot` → PNG | `src/lib.rs` |

Chrome is launched in its **own process group** so its many helper processes
don't keep the launching CLI's pipe/process-group alive.

## Prerequisites & setup

- Google Chrome installed (`/Applications/Google Chrome.app`), Chromium, or
  Chrome Canary — or set `$CHROME_BIN`. No permission prompts; launched
  automatically with an isolated `--user-data-dir`.

## Session / persistence model

`--session <name>` → isolated profile + deterministic debug port. A new invocation
reuses a running Chrome on that port (checked via `GET /json/version`) or launches
one. `@ref`s are the page's `data-abf-ref` attributes. `runtime.endpoint` is the
page WebSocket URL.

## Launch settings

On cold start the window is sized per the shared launch config (see
[session-management.md](../session-management.md#user-config-configtoml)):
default **1360x800**, a ~1.7:1 landscape window. Chrome gets
`--window-size=W,H` (plus `--window-position=X,Y` when set and `--headless=new`
when headless), and the bounds are pinned again over CDP
(`Browser.setWindowBounds`) right after launch. Extra `args` are appended to the
command line. A running Chrome is never resized on resume.

```sh
agent-controller --backend chrome --window-size 1600x900 use example.com
agent-controller --backend chrome --headless --session ci use example.com
```

## Realistic input

`AGENT_CONTROLLER_REALISTIC=1` drives the page the way a person would, for demos
and for recordings someone watches (`src/realistic.rs`). Unset or `0`, every
verb is today's instant input.

| Verb | Realistic | Instant (default) |
|---|---|---|
| click | finds the element, glides a pointer to near its centre with `Input.dispatchMouseEvent` (a curved, eased path with a small wobble; a move over 120 px overshoots by 4-10 px and settles back), waits 120-280 ms, presses and releases | `el.click()` in the page |
| type | one key at a time (`Input.dispatchKeyEvent`), in uneven bursts with a longer gap after spaces, symbols and each burst; a character with no key is inserted as text | `Input.insertText` |
| press | holds the key 25-90 ms | down and up at once |

A drawn cursor (13x17 px, an orange ring on each click) is put into every
document (`Page.addScriptToEvaluateOnNewDocument`, and drawn again after each
call), so a screencast of the tab shows the pointer. It is `aria-hidden`,
`pointer-events:none`, `inert` and has no role, so snapshots and `@ref`s never
include it. It hides when a mouse move the driver did not make comes in. Its
script runs in an isolated world (`agent-controller-cursor`), never the page's:
the page sees the two elements and no global or function. Its state (whether
the driver is moving, where it is) is on the element, since each call is a new
CDP session and so a new world. The
pointer's last place is kept in the session folder (`pointer.json`), so the next
call glides on from there. Nothing brings the window to the front.

`AGENT_CONTROLLER_REALISTIC_SEED=<n>` fixes the timings, for reproducible demos.

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✓ | ✓ | ✗ | ✓ | ✓ |

## Commands & examples

```sh
agent-controller --backend chrome use example.com
agent-controller --backend chrome snapshot
agent-controller --backend chrome click @e2
agent-controller --backend chrome click "css:#search"
agent-controller --backend chrome type "hello"
agent-controller --backend chrome press "Cmd+A"
agent-controller --backend chrome screenshot page.png
agent-controller --backend chrome --session work use github.com
```

## Limitations & troubleshooting

- **Chrome not found** → install Chrome or set `$CHROME_BIN`.
- If launched alongside your everyday Chrome, this uses a separate profile/port,
  so it won't touch your normal browsing session.
- Network interception (`Network.*`) is available in CDP but not yet exposed as a
  `Controller` verb.
