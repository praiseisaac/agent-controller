# android-emu controller

Controls an Android emulator (or a connected device) through **`adb`**.

- Crate: `crates/android-emu` (`agent-controller-android-emu`)
- Backend id: `android-emu`
- Session id: `android-emu/<serial>`
- **Cross-platform**: `adb` runs on macOS/Windows/Linux, so this backend works
  from any host. It's the Android analogue of `ios-sim`.

## Transport & implementation

Everything shells out to `adb` (`src/adb.rs`); the UI tree is parsed from
`uiautomator dump` XML (`src/snapshot.rs`).

| Verb | adb mechanism |
|---|---|
| navigate | URL → `am start -a android.intent.action.VIEW -d <url>`; `pkg/activity` → `am start -n …`; bare package → `monkey -p <pkg> … 1`; `home` → `input keyevent KEYCODE_HOME` |
| snapshot | `uiautomator dump /sdcard/window_dump.xml` + `cat` → parse XML → `@ref` tree (bounds→Rect, text/content-desc→label, class→role, `clickable`→action) |
| click | resolve to bounds center → `input tap x y` |
| type | `input text <text>` (spaces encoded as `%s`) |
| press | `input keyevent KEYCODE_*` (enter/back/home/tab/del/arrows/…) |
| scroll | `input swipe x1 y1 x2 y2 300` (from `wm size` center) |
| screenshot | `exec-out screencap -p` → PNG |

## Prerequisites & setup

- **Android platform-tools** (`adb`) on `PATH` — `brew install android-platform-tools`
  (macOS) or the Android SDK's `platform-tools` (any OS).
- A **running emulator** (Android Studio AVD) or a USB device with debugging on.
  Verify with `adb devices`.

No daemon of ours: the emulator + `adb` server persist; each invocation reconnects.

## Session / persistence model

Keyed by device **serial** (`--session <serial>`; defaults to the single device
from `adb devices`). `refs.json` (snapshot `@ref`s) + `screenshots/` live in the
session dir. Coordinates are device **pixels** (same space `input tap` uses).

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✗ | ✗ | ✗ | ✓ | ✓ |

## Commands & examples

```sh
adb devices                                            # confirm a device/emulator
agent-controller --backend android-emu status
agent-controller --backend android-emu use com.android.settings   # launch by package
agent-controller --backend android-emu use https://example.com    # open a URL
agent-controller --backend android-emu use home
agent-controller --backend android-emu snapshot
agent-controller --backend android-emu click @e5                   # tap by ref
agent-controller --backend android-emu click "Network & internet"  # tap by label
agent-controller --backend android-emu type "hello"
agent-controller --backend android-emu press back
agent-controller --backend android-emu scroll down 800
agent-controller --backend android-emu screenshot out.png
# multiple devices:
agent-controller --backend android-emu --session emulator-5554 snapshot
```

## Limitations & troubleshooting

- **`failed to run adb`** → install platform-tools / put `adb` on `PATH`.
- **`no running device`** → start an AVD; check `adb devices`.
- `uiautomator dump` can fail on screens with `FLAG_SECURE` or mid-animation;
  retry, or fall back to coordinate taps.
- Locators: `@ref`, `"Label"`, `role:<class>`, `(x,y)`. (CSS is browser-only.)
