//! The binary itself, run responsible for itself (disclaim.rs): `__responsibility` re-runs
//! with responsibility disclaimed in any build, so these hold without any grant, and from
//! any caller — one whose responsible process has exited included (fleet's SECAUDIT AC12).
#![cfg(target_os = "macos")]

use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_agent-controller"));
    c.env_remove("AGENT_CONTROLLER_DISCLAIMED");
    c
}

#[test]
fn it_always_runs_again_responsible_for_itself_and_passes_no_marker_on() {
    let out = bin().arg("__responsibility").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rec: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(rec["self_responsible"], true, "{rec}");
    assert_eq!(rec["responsible_pid"], rec["pid"], "{rec}");
    // AC12a: it got there by running again, whatever the caller reads as
    assert_eq!(rec["rerun"], true, "{rec}");
    // AC12b: the marker is out of its environment before the call runs: nothing it starts
    // can inherit it
    assert_eq!(rec["marker_in_env"], false, "{rec}");
}

#[test]
fn a_marker_another_process_set_is_refused_and_nothing_runs() {
    // the test process is this binary's parent, and it is not agent-controller: whatever the
    // marker says — the parent's own pid, launchd's, a word — it is not the binary's own re-run
    let me = std::process::id().to_string();
    for value in [me.as_str(), "1", "yes"] {
        let out = bin()
            .arg("__responsibility")
            .env("AGENT_CONTROLLER_DISCLAIMED", value)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "marker {value:?}");
        assert!(
            out.stdout.is_empty(),
            "nothing ran: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .contains("a marker its own disclaimed run did not set"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
