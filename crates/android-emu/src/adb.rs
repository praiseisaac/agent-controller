//! `adb` shell-out helpers. Works on any host (macOS/Windows/Linux) — `adb` is
//! the cross-platform bridge to a running Android emulator or device.

use agent_controller_core::{anyhow, Result};
use std::process::Command;

/// Run `adb [-s serial] <args...>` and return stdout (text).
pub fn adb(serial: Option<&str>, args: &[&str]) -> Result<String> {
    let out = raw(serial, args)?;
    if !out.status.success() {
        return Err(anyhow!(
            "adb {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run adb and return the raw bytes of stdout (for binary output like screenshots).
pub fn adb_bytes(serial: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
    let out = raw(serial, args)?;
    if !out.status.success() {
        return Err(anyhow!(
            "adb {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn raw(serial: Option<&str>, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("adb");
    if let Some(s) = serial {
        cmd.arg("-s").arg(s);
    }
    cmd.args(args);
    cmd.output().map_err(|e| {
        anyhow!("failed to run adb ({e}). Install Android platform-tools and ensure `adb` is on PATH.")
    })
}

/// Serial of the single running device/emulator, or error if zero/many.
pub fn single_serial() -> Result<String> {
    let out = adb(None, &["devices"])?;
    let serials: Vec<String> = out
        .lines()
        .skip(1) // "List of devices attached"
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            match (it.next(), it.next()) {
                (Some(s), Some("device")) => Some(s.to_string()),
                _ => None,
            }
        })
        .collect();
    match serials.len() {
        1 => Ok(serials.into_iter().next().unwrap()),
        0 => Err(anyhow!(
            "no running Android device/emulator (start an AVD; check `adb devices`)"
        )),
        _ => Err(anyhow!(
            "multiple devices ({}); pass --session <serial>",
            serials.join(", ")
        )),
    }
}
