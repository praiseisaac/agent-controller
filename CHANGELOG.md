# Changelog

All notable changes to `agent-controller` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/) (pre-1.0: minor bumps may break).

The running binary reports its version with `agent-controller --version`
(`version --json` for a machine-readable record with git commit + build date).

## [Unreleased]

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
