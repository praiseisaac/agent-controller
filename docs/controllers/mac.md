# mac controller

Controls any native macOS application through the **Accessibility API**
(AX-tree-first), with synthetic input and screenshots.

- Crate: `crates/mac` (`agent-controller-mac`)
- Backend id: `mac`
- Session id: `mac/<bundle-id-or-app-name>`

## Control model: foreground-only

mac control is **foreground-only** by design — the same way you can't use a
browser window while it's being driven. On every session open the target app is
**activated and we wait until it is actually frontmost** before sending input.
This is required because macOS routes keyboard shortcuts and formatting commands
(e.g. `Cmd+B`) to the *key window's* first responder, which doesn't exist while
an app is in the background.

## Transport & implementation

| Concern | Mechanism | File |
|---|---|---|
| Read UI tree | `AXUIElement` walk (`accessibility` crate) → `@ref` snapshot | `src/ax.rs` |
| Click (semantic) | `AXPress` on the matched element | `src/ax.rs`, `src/lib.rs` |
| Click (ref/point) | `CGEvent` mouse event (global HID tap) | `src/input.rs` |
| Type / shortcuts | **System Events** via `osascript` (reliable; respects typing attributes) | `src/input.rs` |
| Menus | walk `AXMenuBar` by path, `AXPress` the leaf | `src/ax.rs` |
| Scroll | `CGEvent` scroll-wheel | `src/input.rs` |
| Screenshot | `screencapture -l <window-id>` (occlusion-proof) | `src/lib.rs`, `src/window.rs` |
| App launch / pid / activate | `open`, `osascript` (System Events) | `src/appres.rs` |
| Displays / scale | `CGDisplay` (mode-based backing scale) | `src/display.rs` |

> Why System Events for the keyboard: raw `CGEvent` keyboard posting proved
> unreliable on current macOS (even a plain Delete didn't register). System
> Events `keystroke` / `key code` delivers reliably to the frontmost app.

## Prerequisites & setup

1. **Accessibility** permission (read UI tree + send input) — grant it to your
   *terminal app* (e.g. iTerm/Terminal) in System Settings ▸ Privacy & Security ▸
   Accessibility.
2. **Screen Recording** permission (screenshots).

Run `agent-controller doctor` — it reports both, triggers the system prompts, and
deep-links the settings panes.

## Coordinate model (multi-screen / Retina)

AX frames, `CGEvent` input, and `screencapture` all share **one global point
space** (top-left origin of the main display, spanning all monitors; secondary
displays get offset/negative coordinates). AppKit's bottom-left `NSScreen` space
is never used. So **element-based interaction is screen-agnostic** — no per-display
math. Screenshots are device *pixels* (Retina = 2×, can vary per display);
`agent-controller displays` reports each screen's bounds + backing `scale` to map
screenshot pixels → click points.

## Capabilities

| eval | network | menus | coordinates | screenshot |
|---|---|---|---|---|
| ✗ | ✗ | ✓ | ✓ | ✓ |

## Commands & examples

```sh
# select target with --app (bundle id or name); defaults to the frontmost app
agent-controller --backend mac --app Finder snapshot
agent-controller --backend mac --app TextEdit type "hello world"
agent-controller --backend mac --app TextEdit press "Cmd+A"
agent-controller --backend mac --app TextEdit press "Cmd+B"      # bold the selection
agent-controller --backend mac --app TextEdit menu "Format>Font>Bold"
agent-controller --backend mac --app Finder click "Applications"  # AXPress by label
agent-controller --backend mac --app Finder screenshot finder.png
agent-controller displays
```

Locators: `@ref`, `"Label"`, `(x,y)`. (CSS is browser-only.)

## Limitations & troubleshooting

- **Foreground-only:** acting on an app brings it forward. Don't expect to keep
  using the Mac uninterrupted while it runs.
- **AX coverage varies:** AppKit apps expose rich trees; some Electron/custom UI
  expose less. When an element has no `AXPress`, a coordinate click at its frame
  center is used.
- **"Accessibility permission not granted"** → run `agent-controller doctor` and
  enable your terminal app; permission is per-binary/terminal, so re-grant if you
  switch terminals.
- **Blank/occluded screenshot** → ensure Screen Recording is granted; capture is
  by window id so occlusion is handled, but the app must have an on-screen window.
