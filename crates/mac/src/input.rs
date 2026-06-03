//! Synthetic input. mac control is foreground-only (the target app is activated
//! first), so:
//! - keyboard (type / shortcuts) goes through **System Events** (`osascript`),
//!   which reliably delivers keystrokes — and respects typing attributes like
//!   bold — to the frontmost app. (Raw CGEvent keyboard posting proved
//!   unreliable on this macOS.)
//! - mouse clicks / scroll use CGEvent against the now-frontmost window.

use agent_controller_core::{anyhow, Result, ScrollDir};
use core_graphics::event::{
    CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use std::process::Command;

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| anyhow!("failed to create CGEventSource"))
}

pub fn click(_pid: i32, x: f64, y: f64, _takeover: bool) -> Result<()> {
    let pt = CGPoint::new(x, y);
    let src = source()?;
    let down = CGEvent::new_mouse_event(
        src.clone(),
        CGEventType::LeftMouseDown,
        pt,
        CGMouseButton::Left,
    )
    .map_err(|_| anyhow!("mouse down event"))?;
    let up = CGEvent::new_mouse_event(src, CGEventType::LeftMouseUp, pt, CGMouseButton::Left)
        .map_err(|_| anyhow!("mouse up event"))?;
    down.post(CGEventTapLocation::HID);
    up.post(CGEventTapLocation::HID);
    Ok(())
}

pub fn scroll(_pid: i32, dir: ScrollDir, amount: i32, _takeover: bool) -> Result<()> {
    let a = amount.max(1);
    let (v, h) = match dir {
        ScrollDir::Down => (-a, 0),
        ScrollDir::Up => (a, 0),
        ScrollDir::Left => (0, a),
        ScrollDir::Right => (0, -a),
    };
    let src = source()?;
    let ev = CGEvent::new_scroll_event(src, ScrollEventUnit::PIXEL, 2, v, h, 0)
        .map_err(|_| anyhow!("scroll event"))?;
    ev.post(CGEventTapLocation::HID);
    Ok(())
}

fn osascript(script: &str) -> Result<()> {
    let out = Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| anyhow!("osascript: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "keystroke failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Type `text` into the frontmost app via System Events (respects bold/italic
/// typing attributes).
pub fn type_text(text: &str) -> Result<()> {
    osascript(&format!(
        "tell application \"System Events\" to keystroke \"{}\"",
        applescript_escape(text)
    ))
}

/// Press a key spec like `Cmd+Shift+S`, `Enter`, `Tab` via System Events.
pub fn press(spec: &str) -> Result<()> {
    let mut modifiers: Vec<&str> = Vec::new();
    let mut base: Option<String> = None;
    for part in spec.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" | "super" => modifiers.push("command down"),
            "shift" => modifiers.push("shift down"),
            "alt" | "option" | "opt" => modifiers.push("option down"),
            "ctrl" | "control" => modifiers.push("control down"),
            other => base = Some(other.to_string()),
        }
    }
    let base = base.ok_or_else(|| anyhow!("no base key in {spec:?}"))?;
    let using = if modifiers.is_empty() {
        String::new()
    } else {
        format!(" using {{{}}}", modifiers.join(", "))
    };
    let action = if let Some(code) = special_keycode(&base) {
        format!("key code {code}{using}")
    } else if base.chars().count() == 1 {
        format!("keystroke \"{}\"{using}", applescript_escape(&base))
    } else {
        return Err(anyhow!("unknown key: {base}"));
    };
    osascript(&format!("tell application \"System Events\" to {action}"))
}

/// macOS virtual key codes for non-printable keys.
fn special_keycode(name: &str) -> Option<u16> {
    Some(match name {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        _ => return None,
    })
}
