//! App resolution: launch/activate apps and resolve their pids via `open` and
//! `osascript` (System Events). No extra entitlements beyond what AX needs.

use agent_controller_core::{anyhow, Result};
use std::process::Command;
use std::time::Duration;

fn osascript(script: &str) -> Result<String> {
    let out = Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|e| anyhow!("osascript: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "osascript failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn looks_like_bundle(s: &str) -> bool {
    s.contains('.') && !s.contains(' ') && !s.ends_with(".app") && !s.contains('/')
}

/// `(pid, bundle-id)` of the frontmost application process.
pub fn frontmost() -> Result<(i32, String)> {
    let out = osascript(
        "tell application \"System Events\" to get {unix id, bundle identifier of it} of first application process whose frontmost is true",
    )?;
    // e.g. "71408, com.googlecode.iterm2"
    let mut parts = out.splitn(2, ',');
    let pid = parts
        .next()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .ok_or_else(|| anyhow!("could not parse frontmost pid from {out:?}"))?;
    let bundle = parts.next().map(|s| s.trim().to_string()).unwrap_or_default();
    Ok((pid, bundle))
}

/// The process (app) name for a given pid, via System Events.
pub fn name_for_pid(pid: i32) -> Result<String> {
    osascript(&format!(
        "tell application \"System Events\" to get name of (first process whose unix id is {pid})"
    ))
}

/// Bring a specific pid to the foreground and block until it is actually
/// frontmost — the instance-targeted analogue of `activate_and_wait`, used when
/// `--pid` picks one process among several of the same app.
pub fn activate_pid(pid: i32) {
    let _ = osascript(&format!(
        "tell application \"System Events\" to set frontmost of (first process whose unix id is {pid}) to true"
    ));
    for _ in 0..25 {
        if let Ok((fp, _)) = frontmost() {
            if fp == pid {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// Resolve a running app's pid by bundle id or process name.
pub fn pid_for(target: &str) -> Result<i32> {
    let script = if looks_like_bundle(target) {
        format!("tell application \"System Events\" to get unix id of (first process whose bundle identifier is \"{target}\")")
    } else {
        let name = target.strip_suffix(".app").unwrap_or(target);
        format!("tell application \"System Events\" to get unix id of (first process whose name is \"{name}\")")
    };
    osascript(&script)?
        .parse::<i32>()
        .map_err(|_| anyhow!("no running process for {target:?}"))
}

/// Launch or activate `target` (a bundle id, app name, or URL).
pub fn launch(target: &str) -> Result<()> {
    let mut cmd = Command::new("open");
    if target.contains("://") {
        cmd.arg(target);
    } else if looks_like_bundle(target) {
        cmd.args(["-b", target]);
    } else {
        cmd.args(["-a", target.strip_suffix(".app").unwrap_or(target)]);
    }
    let status = cmd.status().map_err(|e| anyhow!("open: {e}"))?;
    if !status.success() {
        return Err(anyhow!("`open` failed for {target:?}"));
    }
    Ok(())
}

/// Activate `target` and block until it is actually frontmost (so subsequent
/// keystrokes land in its key window). Activation via `open` is asynchronous.
pub fn activate_and_wait(target: &str, pid: i32) {
    let _ = launch(target);
    for _ in 0..25 {
        if let Ok((fp, _)) = frontmost() {
            if fp == pid {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

/// Launch `target` if needed and return its pid, polling briefly for startup.
pub fn launch_and_resolve(target: &str) -> Result<i32> {
    if let Ok(pid) = pid_for(target) {
        return Ok(pid);
    }
    launch(target)?;
    for _ in 0..40 {
        if let Ok(pid) = pid_for(target) {
            return Ok(pid);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(anyhow!("launched {target:?} but could not resolve its pid"))
}
