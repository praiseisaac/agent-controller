# safari controller

Controls Safari via **`safaridriver`** (the W3C WebDriver protocol over HTTP).

- Crate: `crates/safari` (`agent-controller-safari`)
- Backend id: `safari`
- Session id: `safari/<instance-name>` (`--session`, default `default`)

## Model

`safaridriver` is a long-lived WebDriver HTTP server. A WebDriver session (and
its Safari window) lives on that server. We keep `safaridriver` running and
**reuse the stored `sessionId`** across CLI invocations; WebDriver is plain
request/response, so no persistent socket of our own is needed.

## Transport & implementation

| Concern | Mechanism | File |
|---|---|---|
| WebDriver HTTP client | `POST /session`, `/url`, `/execute/sync`, `/actions`, `GET /screenshot` | `src/webdriver.rs` |
| safaridriver lifecycle | spawn `safaridriver -p <port>` (own process group), `GET /status` | `src/launch.rs` |
| Navigate | `POST /session/{id}/url` | `src/lib.rs` |
| Snapshot / click | `execute/sync` JS tagging `data-abf-ref` | `src/lib.rs` |
| Type / press | Actions API key input (WebDriver key codepoints) | `src/lib.rs` |
| Scroll | `execute/sync` `window.scrollBy` | `src/lib.rs` |
| Screenshot | `GET /session/{id}/screenshot` → PNG | `src/lib.rs` |

## Prerequisites & setup

**Enable Remote Automation (one-time):**

```sh
safaridriver --enable        # authenticate when prompted
```
…or Safari ▸ Settings ▸ Advanced ▸ "Show features for web developers", then
Develop ▸ **Allow Remote Automation**.

`agent-controller doctor` reports Safari Remote Automation status (it probes a
throwaway WebDriver session and distinguishes "not enabled" from an incidental
busy-session error) and prints the enable instructions when it's off.

## Session / persistence model

`--session <name>` → its own `safaridriver` port + WebDriver session. The
`sessionId` is stored in `session.json` (`config.session_id`) and reused if still
alive; otherwise a new session (and Safari window) is created. `@ref`s are the
page's `data-abf-ref` attributes.

## Launch settings

When a new WebDriver session (and so a new Safari window) is created, the window
is sized per the shared launch config (see
[session-management.md](../session-management.md#user-config-configtoml)):
default **1360x800**, a ~1.7:1 landscape window, applied via
`POST /session/{id}/window/rect` (with `x`/`y` when set). safaridriver owns the
launch, so `headless` and `args` do not apply to Safari. An existing session's
window is never resized on resume.

```sh
agent-controller --backend safari --window-size 1600x900 use example.com
```

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✓ | ✗ | ✗ | ✓ | ✓ |

## Commands & examples

```sh
agent-controller --backend safari use example.com
agent-controller --backend safari snapshot
agent-controller --backend safari click @e2
agent-controller --backend safari click "Learn more"
agent-controller --backend safari type "query"
agent-controller --backend safari press Enter
agent-controller --backend safari screenshot page.png
```

## Limitations & troubleshooting

- **"You must enable 'Allow remote automation'"** → run `safaridriver --enable`
  (and verify the Develop-menu toggle), then `agent-controller doctor`.
- Safari permits a limited number of concurrent automation sessions; a second
  session may fail while one is active.
- No network interception (not part of WebDriver).
