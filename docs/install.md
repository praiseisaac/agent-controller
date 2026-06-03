# Installing

Three ways to install `agent-controller`: a one-liner, the `install.sh` script
from a checkout, or a manual build from source. All build the same release binary.

## One-liner

Clones the repo, builds, and installs to `~/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash
# also brew-install missing build/runtime deps (protoc, idb-companion):
curl -fsSL https://raw.githubusercontent.com/praiseisaac/agent-controller/main/install.sh | bash -s -- --install-deps
```

---

## Build from source

### 1. Prerequisites

- **macOS** (the backends are macOS-specific).
- **Rust** (stable) — install from <https://rustup.rs>.
- **protoc** (Protocol Buffers compiler) — required to build the `ios-sim`
  backend (it compiles `idb.proto` via `tonic-build`):
  ```sh
  brew install protobuf
  ```

Per-backend runtime tools (only for the backends you use):

| Backend | Needs |
|---|---|
| mac | Accessibility + Screen Recording permissions (`agent-controller doctor`) |
| ios-sim | `brew install facebook/fb/idb-companion`; Xcode + a booted simulator |
| firefox | Firefox installed (auto-detected, or `$FIREFOX_BIN`) |
| chrome | Google Chrome installed (auto-detected, or `$CHROME_BIN`) |
| safari | `safaridriver --enable` (or Safari ▸ Develop ▸ Allow Remote Automation) |

### 2. Get the source

```sh
git clone https://github.com/praiseisaac/agent-controller.git
cd agent-controller
```

### 3. Build

```sh
cargo build --release            # whole workspace → target/release/agent-controller
# or a single backend crate while developing:
cargo build -p agent-controller-mac
```

### 4. Install the binary

Pick one:

**a. `install.sh` (recommended)** — handles deps check, stops a running firefox
daemon, and installs with a fresh inode:

```sh
./install.sh                     # → ~/bin/agent-controller
PREFIX=/usr/local/bin ./install.sh
./install.sh --debug             # faster, unoptimized build
```

**b. `cargo install`** — installs to `~/.cargo/bin`:

```sh
cargo install --path app
```

**c. Manual copy** — note the `rm` first (see *Updating* below):

```sh
mkdir -p ~/bin
rm -f ~/bin/agent-controller
cp target/release/agent-controller ~/bin/
```

Make sure the install dir is on your `PATH`:

```sh
export PATH="$HOME/bin:$PATH"     # add to ~/.zshrc
```

### 5. Verify

```sh
agent-controller --help
agent-controller doctor          # checks/guides macOS permissions
agent-controller displays
```

---

## Updating / reinstalling

Always remove the old binary before copying a new one (or just use `install.sh`):

```sh
pkill -f __firefox-daemon          # release the mapped binary, if a daemon is up
rm -f ~/bin/agent-controller
cp target/release/agent-controller ~/bin/
```

> **Why `rm` first:** the firefox daemon runs as `current_exe` =
> `~/bin/agent-controller`, so that file is mapped by a live process. Overwriting
> it in place invalidates its code signature and macOS then kills every run with
> "Killed: 9". `rm`+`cp` (or `install.sh`) gives a fresh inode and avoids this.

## Uninstall

```sh
rm -f ~/bin/agent-controller          # or ~/.cargo/bin/agent-controller
pkill -f __firefox-daemon
rm -rf ~/.agent-controller            # sessions, profiles, screenshots (optional)
```

## Troubleshooting

- **`protoc not found`** during build → `brew install protobuf` (or
  `./install.sh --install-deps`).
- **`Killed: 9` / exit 137** when running the installed binary → it was
  `cp`-overwritten in place while mapped; `rm` it and reinstall.
- **`command not found: agent-controller`** → the install dir isn't on `PATH`
  (see step 4).
- Backend-specific issues → `agent-controller doctor` and the per-controller docs
  under [`controllers/`](controllers/).
