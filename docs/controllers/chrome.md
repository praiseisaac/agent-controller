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
| Type | `Input.insertText` | `src/lib.rs` |
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
