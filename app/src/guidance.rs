//! Operating guidance for driving each backend well — the runtime source of
//! truth surfaced to MCP clients (the `initialize` `instructions` field and the
//! `guidance` tool). The same advice is mirrored as a `## Automation guidance`
//! doc-comment atop each backend crate (`crates/*/src/lib.rs`) so it's visible
//! from the source too; keep the two in sync when either changes.

use agent_controller_core::Backend;

/// Cross-cutting advice that applies to every backend.
pub fn general() -> &'static str {
    "General (all backends):
- Snapshot → act → snapshot. @ref ids come from the most recent `snapshot`; they \
go stale when the UI re-renders (common on React/SPA pages) and a stale ref fails \
with \"no element\". Re-snapshot right before you act, and again to confirm.
- Prefer a `screenshot` to verify what actually happened over inferring from the \
tree or an internal check — the image shows what is *in front*, including native \
dialogs and floating panels that the accessibility tree / window / sheet checks \
miss entirely.
- Locators: `@e3` (from snapshot), `\"Label text\"`, `css:#sel` (browsers only), \
or `(x,y)` points. Call `status` to read a backend's capabilities before using an \
optional verb (`menu`, `upload`)."
}

/// Backend-specific advice.
pub fn for_backend(b: Backend) -> &'static str {
    match b {
        Backend::Mac => "mac — drive any native macOS app via the Accessibility tree.
- Foreground-only: the target is brought to the front and \"owned\" while you \
drive it; you cannot use it concurrently, and focus will move to it.
- Several instances of one app (e.g. multiple Firefox/Chrome windows) share a \
bundle id. Pass `pid` (`--pid <n>`) to bind one specific process; list processes \
to find it. The session is keyed `mac/pid-<n>`.
- `click \"Label\"` / `click role:AXButton` use AXPress (cursor-free). `click \
@ref` / `(x,y)` post a synthetic click.
- Opening a browser or native file picker REQUIRES `takeover` (`--takeover`, a \
real HID click). AXPress and the default non-interruptive click will not open a \
file dialog. `takeover` moves the real cursor.
- Drive a native Open panel entirely from mac: takeover-click the Browse button, \
then `press Cmd+Shift+G`, `type <absolute path>`, `press Enter`, `press Enter`.
- `type`/`press` reach the frontmost app via System Events; formatting chords \
(Cmd+B) apply.
- Verify with `screenshot` (captures the window even when occluded), not by \
inspecting AX windows/sheets — native dialogs are floating panels AX enumeration \
misses.",

        Backend::Firefox => "firefox — WebDriver BiDi via a long-lived per-session daemon.
- Instances are keyed by `session` (`--session <name>`), each with its own \
profile. The first command spawns the daemon, which owns the BiDi session for the \
browser's lifetime.
- Do NOT `pkill __firefox-daemon` while in use — it orphans the session and \
wedges that Firefox (the next command can't re-establish one). Recover by \
restarting that Firefox process.
- File upload: use the `upload @ref <absolute path>` verb (BiDi input.setFiles) — \
in-band, no OS picker, no focus stealing, works even on a background tab. Do not \
try to open the native dialog.
- @ref ids are data-abf-ref DOM attributes; re-snapshot after navigation or a \
re-render.",

        Backend::Chrome => "chrome — Chrome DevTools Protocol (CDP); no daemon (transient connections per session).
- File upload: `upload @ref <absolute path>` (CDP DOM.setFileInputFiles) — \
in-band, no OS picker, no focus stealing.
- Same stale-@ref rule: re-snapshot before acting after the page changes.",

        Backend::Safari => "safari — safaridriver (W3C WebDriver). Requires Remote Automation enabled (run `doctor`).
- No in-band upload verb yet (capabilities.upload = false). For a file <input>, \
fall back to the mac backend's native-dialog route with `takeover`.",

        Backend::IosSim => "ios-sim — a booted iOS Simulator via idb_companion.
- Coordinates are logical points, and screenshots are downscaled to that same \
point space, so a coordinate read off a screenshot maps 1:1 to a tap.
- `udid` (`--udid`) selects among booted sims (defaults to the only one). \
`navigate` launches a bundle id, opens a URL, or goes `home`.",

        Backend::Windows => "windows — UI Automation + SendInput.
- Binds to the foreground window, or pass `pid` (`--pid <n>`) for a specific \
process's main window.
- No in-band upload (native file dialogs only); drive them with keystrokes like \
the mac route.",

        Backend::AndroidEmu => "android-emu — adb + uiautomator.
- `udid`/serial selects a device when more than one is attached. Cross-platform \
(any host with adb on PATH).",
    }
}

/// Full guidance: general advice plus every backend's notes. Returned by the
/// `guidance` tool when no specific backend is requested.
pub fn all() -> String {
    let mut s = String::from(general());
    for b in [
        Backend::Mac,
        Backend::Windows,
        Backend::Firefox,
        Backend::Chrome,
        Backend::Safari,
        Backend::IosSim,
        Backend::AndroidEmu,
    ] {
        s.push_str("\n\n");
        s.push_str(for_backend(b));
    }
    s
}
