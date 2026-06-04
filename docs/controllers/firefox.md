# firefox controller

Controls Firefox over **WebDriver BiDi** (the modern, standardized bidirectional
protocol). Vendored from `agent-browser-firefox`.

- Crate: `crates/firefox` (`agent-controller-firefox`)
- Backend id: `firefox`
- Session id: `firefox/<instance-name>` (`--session`, default `default`)

## Why a daemon

A BiDi session is **bound to its WebSocket connection** — when the connection
closes, Firefox ends the session and a fresh connection cannot reclaim it (it
hits "maximum number of active sessions"). Since each CLI invocation is ephemeral,
a long-lived **daemon** owns the WebSocket + session for the browser's lifetime;
CLI invocations are thin clients over a localhost TCP socket.

```
CLI (FirefoxController)  ──line-JSON over localhost TCP──▶  firefox daemon  ──BiDi ws──▶  Firefox
   navigate/snapshot/...                                  owns BidiSession            (isolated profile)
```

## Transport & implementation

| Concern | Mechanism | File |
|---|---|---|
| Daemon (owns ws + session) | `agent-controller __firefox-daemon` (hidden), localhost-TCP server | `src/daemon.rs`, `src/ipc.rs` |
| BiDi WebSocket client | multiplexed id/oneshot routing | `src/bidi/client.rs` |
| BiDi session ops | navigate/evaluate/screenshot/input | `src/bidi/session.rs` |
| Launch + endpoint discovery | `--remote-debugging-port`, poll TCP, isolated profile | `src/launch.rs` |
| Snapshot / click | injected JS tagging `data-abf-ref` (via `eval`) | `src/lib.rs` |

The daemon exposes thin ops (`navigate`, `eval`, `type`, `press`, `scroll`,
`screenshot`); the controller builds the snapshot/click JS and calls `eval`.

## Prerequisites & setup

- Firefox installed (Nightly / Developer Edition / stable, or `$FIREFOX_BIN`).
  Resolved automatically; no permission prompts.
- No manual setup. The daemon + an isolated automation profile are created on
  first use (telemetry/first-run/update nags disabled via a generated `user.js`).

## Session / persistence model

`--session <name>` → an isolated profile + deterministic debug port + daemon, all
under the session dir. Multiple named instances coexist
(`--session work`, `--session personal`). `@ref`s are the page's `data-abf-ref`
attributes (persist in the live DOM across invocations), so no `refs.json` cache.
`runtime.endpoint` is the daemon socket.

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✓ | ✓ | ✗ | ✓ | ✓ |

## Commands & examples

```sh
agent-controller --backend firefox use example.com     # launches Firefox + navigates
agent-controller --backend firefox snapshot
agent-controller --backend firefox click @e2           # by ref
agent-controller --backend firefox click "Learn more"  # by text
agent-controller --backend firefox click "css:#submit" # by CSS selector
agent-controller --backend firefox type "query"
agent-controller --backend firefox press Enter
agent-controller --backend firefox screenshot page.png
agent-controller --backend firefox --session work use github.com   # second instance
```

## Limitations & troubleshooting

- **Daemon lifecycle:** the daemon is `current_exe`. If you reinstall the binary
  while a daemon runs, the old daemon keeps the old binary; `pkill -f
  __firefox-daemon` to refresh. (See the repo's install note about `rm`+`cp`.)
- **Daemon not ready** → check `/tmp/agent-controller/firefox-daemon-<name>.log`.
- Network interception / console logs exist in the vendored session but aren't
  exposed as `Controller` verbs yet.
