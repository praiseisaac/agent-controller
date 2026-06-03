# Contributing

Thanks for your interest in `agent-controller`!

## Project shape

A Cargo workspace: a backend-agnostic `agent-controller-core` defines the
`Controller` trait + factory interface + session store; each backend is its own
crate; the `agent-controller` binary wires them together (CLI + MCP server). See
[docs/architecture.md](docs/architecture.md).

## Add a new backend

This is the most common contribution — teaching the tool to drive a new target
(an app, device, emulator, or browser). It's two trait impls in a new crate plus
a one-line registry edit; the CLI and MCP server then support it for free.

→ **[docs/adding-a-controller.md](docs/adding-a-controller.md)** (step-by-step,
with a copy-pasteable skeleton).

## Dev workflow

```sh
cargo build                 # whole workspace
cargo build -p agent-controller-<backend>
./install.sh --debug        # build + install to ~/bin for manual testing
agent-controller doctor     # check macOS permissions
```

## Conventions

- Keep `cargo build` warning-free.
- `snapshot` returns the shared outline (`[@e1] role "label" (x,y WxH)`) with
  stable `@eN` refs; resolve those in `click`.
- Reflect what a backend can do in `capabilities()`; return clear errors for
  unsupported locators/verbs.
- Detach long-lived child processes into their own process group; never leave the
  user's foreground app/cursor hijacked beyond what the backend's model requires.
- Reinstall with `rm`+`cp` (or `./install.sh`), never an in-place `cp` over a
  running binary — see [the install note](README.md#chrome-backend).
- Add/update the backend's doc under `docs/controllers/` and the tables in
  `README.md` and `docs/README.md`.

## Verifying a change

Prefer driving a real target end-to-end (`use` → `snapshot` → `click` →
`screenshot`) over unit tests alone — these backends are mostly I/O against live
apps. Note in your PR which targets you verified against (OS/app versions).
