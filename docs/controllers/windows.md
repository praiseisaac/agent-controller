# windows controller

Controls any Windows desktop application through **UI Automation**
(`IUIAutomation`) for reading/invoking, and **`SendInput`** for synthetic input.

- Crate: `crates/windows` (`agent-controller-windows`)
- Backend id: `windows`
- Session id: `windows/<app-or-foreground>`
- **Windows-only**: compiles to an empty crate on other platforms. Build and run
  it on Windows.

> Status: first cut. The `windows`-crate COM/UIA surface is version-sensitive;
> expect to iterate against the compiler on your machine. See "Building" below.

## Transport & implementation (`src/lib.rs`)

| Verb | Mechanism |
|---|---|
| snapshot | `IUIAutomation` control-view walk of the foreground window → `@ref` (ControlType→role, Name→label, BoundingRectangle→Rect, InvokePattern→action) |
| click | `InvokePattern.Invoke()` (by label/role) → fallback `SendInput` mouse at the element/point center |
| type | `SendInput` Unicode keystrokes (`KEYEVENTF_UNICODE`) into the focused control |
| press | `SendInput` virtual-key events (+ Ctrl/Shift/Alt/Win modifiers) |
| scroll | `SendInput` mouse wheel |
| screenshot | GDI `BitBlt` of the window (or full screen) → PNG via the `image` crate |
| navigate | `cmd /C start <target>` (app/URL), then waits for the launched window to reach the foreground and settle (avoids the follow-up-command focus race) |

COM objects are apartment-threaded (not `Send`/`Sync`) while `Controller` is
`Send + Sync`, so the controller stores only the target window handle as an
integer; each method initializes COM and builds the automation object on its own
thread (fine for a one-shot CLI).

## Prerequisites & setup

- **Rust** (rustup) with the MSVC toolchain (`x86_64-pc-windows-msvc`).
- No `protoc` needed (the `ios-sim` backend that requires it is macOS-only and
  not built on Windows).
- No special permission like macOS Accessibility — UI Automation works out of the
  box. **Caveat:** a non-elevated process cannot drive **elevated** windows (run
  elevated to automate elevated apps).

## Building (on Windows)

```powershell
git clone https://github.com/praiseisaac/agent-controller.git
cd agent-controller
cargo build --release            # builds windows + firefox + chrome + android-emu
.\target\release\agent-controller.exe --backend windows snapshot
```

### Adding to PATH

```powershell
# PowerShell (current user, persistent)
$bin = "$env:USERPROFILE\.agent-controller\bin"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
Copy-Item .\target\release\agent-controller.exe $bin
[Environment]::SetEnvironmentVariable("Path", "$bin;" + [Environment]::GetEnvironmentVariable("Path", "User"), "User")
# restart your terminal for the change to take effect
```

```bat
rem Command Prompt (current user, persistent)
set "BIN=%USERPROFILE%\.agent-controller\bin"
mkdir "%BIN%" 2>nul
copy /Y target\release\agent-controller.exe "%BIN%"
setx PATH "%BIN%;%PATH%"
rem restart your terminal for the change to take effect
```

```bash
# Git Bash / MSYS2
BIN="$HOME/.agent-controller/bin"
mkdir -p "$BIN"
cp target/release/agent-controller.exe "$BIN/"
echo 'export PATH="$HOME/.agent-controller/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

The mac/ios-sim/safari crates are skipped on Windows (cfg-gated to empty). If the
build hits `windows`-crate API mismatches, share the errors and we'll adjust the
imports/signatures (they vary by `windows` crate version).

## Session / persistence model

Binds to the **foreground window** on open (a future improvement: resolve a
window by title / `--app`). `refs.json` (snapshot `@ref`s) + `screenshots/` live
in the session dir. Coordinates are screen **pixels**.

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✗ | ✗ | ✗ | ✓ | ✓ |

## Commands & examples

```powershell
agent-controller --backend windows snapshot
agent-controller --backend windows click "Save"          # UIA Invoke by name
agent-controller --backend windows click @e7             # by ref (cached)
agent-controller --backend windows type "hello"
agent-controller --backend windows press "Ctrl+S"
agent-controller --backend windows screenshot out.png
```

## Limitations & roadmap

- First-cut window targeting = foreground window; add title/app resolution +
  per-window screenshots next.
- Horizontal scroll, `ValuePattern.SetValue` (cursor-free text), and richer role
  mapping are TODO.
- Can't drive elevated windows from a non-elevated process (Windows UIPI).
