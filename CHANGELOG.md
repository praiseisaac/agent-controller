# Changelog

All notable changes to `agent-controller` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: minor bumps may break).

The running binary reports its version with `agent-controller --version`
(`version --json` for a machine-readable record with git commit + build date).

## [Unreleased]

### Added
- **Chrome: realistic input (`AGENT_CONTROLLER_REALISTIC=1`).** Clicks glide a
  pointer along a curved, eased path (long moves overshoot and settle) and wait
  a short beat before pressing with real mouse events; typing comes one key at a
  time in an uneven rhythm; a drawn cursor, hidden from snapshots, shows in a
  screencast and hides when the person's own pointer moves. Off by default, and
  off is the instant input it was. `AGENT_CONTROLLER_REALISTIC_SEED` fixes the
  timings.
- **`AGENT_CONTROLLER_BUILD_SHA`.** A build of source vendored into another
  repository names its commit with this variable. `git` is not asked, so the
  binary reports the vendored commit, not the host repository's. Anything but
  7-40 lowercase hex digits fails the build.
- **`AGENT_CONTROLLER_DISCLAIM=1` (at build time): the grants it uses are its own.**
  macOS judges Accessibility and Screen Recording on a process's *responsible*
  process. Started by an orchestrator's session, that is whatever launched the
  session, whose grant covers every process the session runs. A binary built
  with this variable runs every call again with responsibility disclaimed, except
  a closed list that touches no grant (`version`, `sessions`, `session`,
  `config`), so the grant is agent-controller's own, at its own path. A call
  added later disclaims too, unless it is added to that list. The decision is fixed when the binary
  is built, never at run time. The loop guard is the state itself: a re-run that
  is still not responsible for itself, or a caller that presets the marker, is
  refused and nothing runs. Without the variable, nothing changes. Hidden
  `__responsibility` prints whose grant a call would use (a read).
- **`doctor --no-prompt`.** Reads the permissions `--backend` needs and never
  asks for anything: preflight calls only (`AXIsProcessTrusted` with no options,
  `CGPreflightScreenCaptureAccess`). It makes no request, does not open System
  Settings, and brings up no window. Each check not run is listed as skipped,
  with why. The Safari Remote Automation probe opens a Safari window, so it runs
  only with `--probe-safari`. `--json` prints `{backend, prompted: false, checks,
  skipped, ready}`. For callers running unattended (test runners,
  orchestrators). The plain `doctor` is unchanged.

## [0.2.0] - 2026-09-24

### Added
- **Browser launch settings.** chrome/firefox/safari cold-start in a 1360x800
  (~1.7:1) window by default. Configurable via `<home>/config.toml`
  (`[launch]`, `[launch.<backend>]`), env (`AGENT_CONTROLLER_WINDOW_SIZE`,
  `AGENT_CONTROLLER_WINDOW_POSITION`, `AGENT_CONTROLLER_HEADLESS`), and flags
  (`--window-size`, `--window-position`, `--headless`; MCP `window_size` /
  `headless`). One configured dimension derives the other at 1.7:1.
- `config show|init|path` subcommands.
- **Versioning.** `--version` / `version` print the semver, git commit (with
  `-dirty`), build date and platform; `version --json` and `status --json`
  carry the same; the MCP `serverInfo` includes `git` and `build_date`.
  Release procedure in `CONTRIBUTING.md`.

### Changed
- `session.json` for browser sessions records the `launch` settings the running
  browser was started with.
- Workspace version bumped from 0.1.0 to 0.2.0.

## [0.1.0] - 2026-06-05

Initial release: the backend-agnostic `Controller` trait + factory + session
store; `mac`, `ios-sim`, `firefox`, `chrome`, `safari`, `android-emu`, `windows`
backends; CLI (`use/snapshot/click/type/press/scroll/screenshot/menu/upload/
status`, sessions, `displays`, `doctor`); MCP server; `--pid` instance targeting;
in-band file upload; firefox daemon self-heal; `install.sh`.

[Unreleased]: https://github.com/praiseisaac/agent-controller/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/praiseisaac/agent-controller/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/praiseisaac/agent-controller/releases/tag/v0.1.0
