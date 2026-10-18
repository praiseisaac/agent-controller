//! The binary itself, run responsible for itself (disclaim.rs): `__responsibility` re-runs
//! with responsibility disclaimed in any build, so these hold without any grant.
#![cfg(target_os = "macos")]

use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-controller"));
    c.env_remove("AGENT_CONTROLLER_DISCLAIMED");
    c
}

#[test]
fn it_runs_responsible_for_itself() {
    let out = bin().arg("__responsibility").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rec: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rec["self_responsible"], true, "{rec}");
    assert_eq!(rec["responsible_pid"], rec["pid"], "{rec}");
    assert_eq!(rec["rerun"], true, "it got there by running again: {rec}");
}

#[test]
fn a_marker_set_by_the_caller_is_refused_not_trusted() {
    // a caller that sets the loop guard itself would otherwise run on its own responsible
    // process's grant: it is refused instead, and nothing runs
    let out = bin()
        .arg("__responsibility")
        .env("AGENT_CONTROLLER_DISCLAIMED", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "nothing ran: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("still holds another process responsible")
    );
}
