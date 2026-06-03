# Session management — design overview

## Goal

Give `agent-controller` **persistent, auto-resuming sessions, one per controllable
item**. A session is not an arbitrary name you invent — it *is* the item: a
specific simulator (by UDID), a specific browser instance (by profile), a mac app
(by bundle id). Re-running against the same item resumes its session. Each session
owns its config, its live backend process, its `@ref` cache, and its **artifacts**
(screenshots, pulled/pushed files, logs).

Today the only persisted state is the ios-sim ref cache in the temp dir; this
generalizes it into a first-class, per-item store.

## Session identity = backend + instance key

The session id is `"<backend>/<instance-key>"`, where the instance key is whatever
naturally and stably identifies one item of that type:

| Backend  | Instance key                | Resolves from |
|----------|-----------------------------|---------------|
| ios-sim  | simulator **UDID**          | `--udid`, else the single booted sim |
| mac      | app **bundle id** (or pid)  | `--app` / bundle id |
| chrome   | **profile/instance name**   | `--session` (default `default`) → isolated profile + debug port |
| firefox  | **profile/instance name**   | `--session` (default `default`) |
| safari   | **instance name**           | `--session` (default `default`) |

Items with a natural identity (a sim UDID, a mac bundle id) need no naming — the
session is intrinsic and resumes automatically. Items you can run many of
(browsers) take an instance name that maps to an isolated profile, so
`--session work` and `--session personal` are two separate, independently
resumable Chrome sessions. The factory derives the id *before* touching disk
(e.g. resolves the booted UDID), so the right session dir is found deterministically.

## Where it lives

Resolution order (first match wins): `--home`/`AGENT_CONTROLLER_HOME` → a
project-local `.agent-controller/` found by walking up from cwd (like `.git`) →
`~/.agent-controller/`.

```
.agent-controller/
├── config.toml
└── sessions/
    ├── ios-sim/
    │   └── ABE64737-51F2-.../          # one dir per simulator
    │       ├── session.json            # source of truth
    │       ├── refs.json               # last snapshot: @ref → element
    │       ├── screenshots/            # capture history, timestamped
    │       ├── files/                  # files pulled from / pushed to the device
    │       └── companion.log
    ├── chrome/
    │   ├── default/
    │   │   ├── session.json
    │   │   ├── refs.json
    │   │   ├── screenshots/
    │   │   ├── downloads/
    │   │   ├── profile/                # the isolated browser profile
    │   │   └── daemon.log
    │   └── work/...
    ├── firefox/<instance>/...
    └── mac/com.apple.Preferences/...
```

`sessions ls` reads `sessions/*/*/session.json` (grouped by type) — no index file
to drift.

## What a session records

```jsonc
// sessions/ios-sim/<udid>/session.json
{
  "id": "ios-sim/ABE64737-51F2-...",
  "backend": "ios-sim",
  "target": "ABE64737-51F2-...",          // udid | url | bundle-id | app
  "created_at": 1733180000,
  "last_used_at": 1733180500,
  "runtime": {                             // how to re-attach to the live backend
    "endpoint": "http://127.0.0.1:11423",  // companion gRPC / daemon socket
    "pids": [70123],                        // processes WE manage
    "alive": true
  },
  "config": { "grpc_port": 11423 },         // backend-specific, opaque to core
  "last": { "screen": "Settings ▸ General", "snapshot_at": 1733180500 },
  "artifacts": { "screenshots": 12, "files": 3 }  // counts/breadcrumbs
}
```

`config` is an opaque `serde_json::Value` to core — each backend defines its shape
(ios-sim: gRPC port; browsers: profile dir, debug port, headless; mac: pid/bundle).

## Artifacts are filed per session

The store hands each backend per-session artifact directories, and the default
output paths point there:

- **Screenshots** — `screenshot` with no path writes
  `sessions/<backend>/<key>/screenshots/<timestamp>.png` and returns the path
  (and in `--json`). A `--path` still overrides. So capture history accrues per
  item automatically.
- **Files** — backends that can move files (ios-sim `pull`/`push`, browser
  downloads) land them in the session's `files/` (or `downloads/`).
- **refs.json / logs** — the `@ref` cache and backend process log live here too.

Core API:

```rust
impl SessionStore {
    fn screenshots_dir(&self, id: &str) -> PathBuf;  // ensured to exist
    fn files_dir(&self, id: &str) -> PathBuf;
    fn refs_path(&self, id: &str) -> PathBuf;
    fn log_path(&self, id: &str) -> PathBuf;
}
```

## Core API

```rust
pub struct SessionStore { home: PathBuf }
impl SessionStore {
    pub fn discover() -> Result<Self>;            // applies the resolution order
    pub fn load(&self, id: &str) -> Result<Option<SessionRecord>>;
    pub fn save(&self, rec: &SessionRecord) -> Result<()>;   // atomic: temp + rename
    pub fn list(&self) -> Result<Vec<SessionRecord>>;        // walks sessions/*/*
    pub fn remove(&self, id: &str) -> Result<()>;            // record + artifacts
    pub fn lock(&self, id: &str) -> Result<SessionLock>;     // per-session advisory flock
    // + the artifact-path helpers above
}
```

Writes are atomic (temp + rename); mutations take a per-session lock so concurrent
invocations on the *same* item don't race, while different items never block.

## How it plugs into the trait + factory

`Controller` stays pure (UI actions). Persistence wraps it:

```rust
#[async_trait]
pub trait BackendFactory {
    fn backend(&self) -> Backend;
    /// Derive the session id from inputs WITHOUT spawning (e.g. resolve booted udid).
    async fn session_id(&self, opts: &Options) -> Result<String>;
    /// Resume if runtime is live, else cold-start; update runtime in `rec`.
    async fn open(&self, rec: &mut SessionRecord, store: &SessionStore)
        -> Result<Box<dyn Controller>>;
}
```

`factory::create` becomes session-aware:

```
discover store
  → backend.session_id(opts)            // "ios-sim/<udid>" or "chrome/work"
  → store.load(id)  (or new record)
  → backend.open(&mut rec, &store)      // reuse live process, or spawn + record
  → store.save(rec)  → Box<dyn Controller>
```

The controller carries a `SessionHandle { store, id }` so it writes breadcrumbs as
it works: `snapshot()` → `refs.json` + `last.snapshot_at`; `screenshot()` →
`screenshots/`; every command bumps `last_used_at`.

## Liveness & reuse (the payoff)

`runtime.endpoint`/`pids` let a new invocation **re-attach to the already-running
backend** rather than cold-starting:

- ios-sim: probe the recorded gRPC port; reuse if the companion answers, else
  respawn and rewrite `runtime`. (Replaces today's UDID-hash port + blind probe
  with a recorded per-session port.)
- browsers (later): recorded daemon socket/pid, mirroring agent-browser-firefox's
  instance registry, plus the isolated `profile/` under the session.

`sessions prune` reconciles dead `pids`/`endpoint`s (mark `alive:false` or remove
with `--max-age`).

## CLI surface

```
# globals
--session <name>   # only meaningful for multi-instance backends (browsers); default "default"
--udid <udid>      # ios-sim instance selector
--app <bundle>     # mac instance selector
--backend <name>   --home <dir>

# management
agent-controller sessions                  # ls grouped by type: backend/key · target · alive · last-used
agent-controller session show <id>
agent-controller session rm   <id>         # removes record + artifacts
agent-controller session prune [--max-age 7d]
agent-controller session path <id>         # print the session dir (for scripting)
```

`use <target>` updates the current session's `target`. Two simulators or two named
browser profiles are fully independent sessions with their own live process, ref
cache, and artifact history.

## What changes in the current code

- **new** `core::session` (`SessionStore`, `SessionRecord`, `Runtime`,
  `SessionLock`, artifact-dir helpers).
- **ios-sim**: implement `BackendFactory` (`session_id` = `ios-sim/<resolved-udid>`);
  move the temp-dir ref cache to `store.refs_path(id)`; default screenshots into
  `store.screenshots_dir(id)`; record companion port/pid in `runtime`.
- **app**: session-aware `factory::create`; `--session`/`--home`/`--app` globals;
  `sessions`/`session` subcommands; a `Backend → &dyn BackendFactory` registry.

## Non-goals / cautions

- No secrets in `session.json` (targets/URLs only).
- Per-session locks, never global — different items never block each other.
- Project-local discovery is opt-in via a present `.agent-controller/` dir.
- `session rm` deletes artifacts too — confirm or require `--force`.
```
