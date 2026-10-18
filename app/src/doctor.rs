//! `doctor --no-prompt`: what this Mac has granted, read without asking for anything.
//!
//! The plain `doctor` is a guide for a person at the keyboard: when a permission is
//! missing it triggers the system prompt and opens System Settings, and it probes
//! Safari, which briefly opens a Safari window. A caller that runs unattended (a test
//! runner, an orchestrator) must never cause any of that on someone's screen. So this
//! path makes preflight reads only — `AXIsProcessTrusted` with no options and
//! `CGPreflightScreenCaptureAccess` — through [`Preflight`], which has no way to
//! request or open anything. The Safari probe runs only with `--probe-safari`.
//!
//! The checks follow `--backend`: each backend is checked for what it uses, and every
//! check not run is listed as skipped, with why.

use agent_controller_core::Backend;
use serde_json::{json, Value};

/// One permission `doctor` can look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Accessibility,
    ScreenRecording,
    SafariRemoteAutomation,
}

impl Check {
    pub const ALL: [Check; 3] = [
        Check::Accessibility,
        Check::ScreenRecording,
        Check::SafariRemoteAutomation,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Check::Accessibility => "accessibility",
            Check::ScreenRecording => "screen_recording",
            Check::SafariRemoteAutomation => "safari_remote_automation",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Check::Accessibility => "Accessibility",
            Check::ScreenRecording => "Screen Recording",
            Check::SafariRemoteAutomation => "Safari Remote Automation",
        }
    }
}

/// The reads the no-prompt path may make. Deliberately read-only: nothing here can
/// trigger a prompt, open System Settings or bring up a window.
pub trait Preflight {
    /// `AXIsProcessTrusted()`, no options.
    fn accessibility(&self) -> bool;
    /// `CGPreflightScreenCaptureAccess()`.
    fn screen_recording(&self) -> bool;
}

/// Which checks run for `backend`, and why each other one is skipped.
pub fn plan(
    backend: Backend,
    probe_safari: bool,
    macos: bool,
) -> (Vec<Check>, Vec<(Check, String)>) {
    let name = backend.as_str();
    let mut run = Vec::new();
    let mut skipped = Vec::new();
    for check in Check::ALL {
        let used = match check {
            Check::Accessibility | Check::ScreenRecording => backend == Backend::Mac,
            Check::SafariRemoteAutomation => backend == Backend::Safari,
        };
        if !macos {
            skipped.push((check, "not macOS".to_string()));
        } else if !used {
            skipped.push((check, format!("the {name} backend does not use it")));
        } else if check == Check::SafariRemoteAutomation && !probe_safari {
            skipped.push((
                check,
                "probing it opens a Safari window: pass --probe-safari to run it".to_string(),
            ));
        } else {
            run.push(check);
        }
    }
    (run, skipped)
}

/// The record `doctor --no-prompt --json` prints. `safari` is the probe's answer
/// `(safaridriver_present, remote_automation_enabled)`, when it was run.
pub fn report(
    backend: Backend,
    run: &[Check],
    skipped: &[(Check, String)],
    pre: &dyn Preflight,
    safari: Option<(bool, bool)>,
) -> Value {
    let mut checks = serde_json::Map::new();
    for &check in run {
        let ok = match check {
            Check::Accessibility => pre.accessibility(),
            Check::ScreenRecording => pre.screen_recording(),
            Check::SafariRemoteAutomation => {
                safari.map(|(present, on)| present && on).unwrap_or(false)
            }
        };
        checks.insert(check.key().to_string(), Value::Bool(ok));
    }
    // ready: every check the backend needs ran and passed; unknown (null) when one it
    // needs was skipped (the Safari probe without --probe-safari)
    let needed_skipped = skipped.iter().any(|(c, why)| {
        *c == Check::SafariRemoteAutomation
            && backend == Backend::Safari
            && why.contains("--probe-safari")
    });
    let all_ok = checks.values().all(|v| v == &Value::Bool(true));
    json!({
        "backend": backend.as_str(),
        "prompted": false,
        "checks": checks,
        "skipped": skipped.iter().map(|(c, why)| json!({ "check": c.key(), "why": why })).collect::<Vec<_>>(),
        "ready": if needed_skipped { Value::Null } else { Value::Bool(all_ok) },
    })
}

/// The same record, for a person reading a terminal.
pub fn text(rec: &Value) -> String {
    let mark = |b: bool| if b { "granted ✓" } else { "NOT granted ✗" };
    let mut out = format!(
        "agent-controller doctor --no-prompt — {} backend (nothing was asked for, nothing was opened)\n",
        rec["backend"].as_str().unwrap_or("?")
    );
    for check in Check::ALL {
        if let Some(ok) = rec["checks"][check.key()].as_bool() {
            out.push_str(&format!("  {:<26}: {}\n", check.label(), mark(ok)));
        }
    }
    for s in rec["skipped"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "  skipped {}: {}\n",
            s["check"].as_str().unwrap_or("?"),
            s["why"].as_str().unwrap_or("")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A Mac with a chosen answer for each read, counting the reads.
    struct Fake {
        ax: bool,
        screen: bool,
        reads: Cell<u32>,
    }
    impl Preflight for Fake {
        fn accessibility(&self) -> bool {
            self.reads.set(self.reads.get() + 1);
            self.ax
        }
        fn screen_recording(&self) -> bool {
            self.reads.set(self.reads.get() + 1);
            self.screen
        }
    }

    const BACKENDS: [Backend; 7] = [
        Backend::Mac,
        Backend::Windows,
        Backend::Firefox,
        Backend::Chrome,
        Backend::Safari,
        Backend::IosSim,
        Backend::AndroidEmu,
    ];

    #[test]
    fn every_check_is_run_or_listed_as_skipped() {
        for backend in BACKENDS {
            for probe in [false, true] {
                for macos in [false, true] {
                    let (run, skipped) = plan(backend, probe, macos);
                    let mut seen: Vec<Check> = run
                        .iter()
                        .copied()
                        .chain(skipped.iter().map(|(c, _)| *c))
                        .collect();
                    seen.sort_by_key(|c| c.key());
                    let mut all = Check::ALL.to_vec();
                    all.sort_by_key(|c| c.key());
                    assert_eq!(seen, all, "{backend:?} probe={probe} macos={macos}");
                }
            }
        }
    }

    #[test]
    fn the_safari_probe_runs_only_when_asked() {
        for backend in BACKENDS {
            let (run, _) = plan(backend, false, true);
            assert!(!run.contains(&Check::SafariRemoteAutomation), "{backend:?}");
        }
        assert_eq!(
            plan(Backend::Safari, true, true).0,
            vec![Check::SafariRemoteAutomation]
        );
        // unknown, not "ready", when the backend needs the probe and it was not run
        let (run, skipped) = plan(Backend::Safari, false, true);
        let pre = Fake {
            ax: true,
            screen: true,
            reads: Cell::new(0),
        };
        assert_eq!(
            report(Backend::Safari, &run, &skipped, &pre, None)["ready"],
            Value::Null
        );
    }

    #[test]
    fn checks_follow_the_backend() {
        assert_eq!(
            plan(Backend::Mac, false, true).0,
            vec![Check::Accessibility, Check::ScreenRecording]
        );
        for backend in [
            Backend::Chrome,
            Backend::Firefox,
            Backend::IosSim,
            Backend::AndroidEmu,
            Backend::Windows,
        ] {
            let (run, skipped) = plan(backend, true, true);
            assert!(run.is_empty(), "{backend:?}");
            assert!(
                skipped
                    .iter()
                    .all(|(_, why)| why.contains("does not use it")),
                "{backend:?}"
            );
        }
        assert!(plan(Backend::Mac, true, false).0.is_empty());
    }

    #[test]
    fn the_report_reads_only_what_it_runs() {
        let pre = Fake {
            ax: true,
            screen: false,
            reads: Cell::new(0),
        };
        let (run, skipped) = plan(Backend::Mac, false, true);
        let rec = report(Backend::Mac, &run, &skipped, &pre, None);
        assert_eq!(
            rec["checks"],
            json!({ "accessibility": true, "screen_recording": false })
        );
        assert_eq!(rec["prompted"], json!(false));
        assert_eq!(rec["ready"], json!(false));
        assert_eq!(pre.reads.get(), 2);
        assert!(text(&rec).contains("Screen Recording          : NOT granted ✗"));
        // a backend that uses neither never reads either
        let pre = Fake {
            ax: true,
            screen: true,
            reads: Cell::new(0),
        };
        let (run, skipped) = plan(Backend::Chrome, false, true);
        assert_eq!(
            report(Backend::Chrome, &run, &skipped, &pre, None)["ready"],
            json!(true)
        );
        assert_eq!(pre.reads.get(), 0);
    }

    /// The no-prompt path never names a call that prompts, opens or raises a window:
    /// not in this module, and not in the main.rs glue that runs it.
    #[test]
    fn no_request_or_open_call_on_the_no_prompt_path() {
        const FORBIDDEN: [&str; 8] = [
            "ax_request",
            "screen_recording_request",
            "AXIsProcessTrustedWithOptions",
            "kAXTrustedCheckOptionPrompt",
            "CGRequestScreenCaptureAccess",
            "open_settings",
            "x-apple.systempreferences",
            "Command::new(\"open\")",
        ];
        let module = include_str!("doctor.rs");
        // this module minus its own test list
        let code = module.split("#[cfg(test)]").next().unwrap();
        let main = include_str!("main.rs");
        let glue = main
            .split("// ── doctor --no-prompt ──")
            .nth(1)
            .expect("main.rs marks the no-prompt glue")
            .split("// ── end doctor --no-prompt ──")
            .next()
            .unwrap();
        for name in FORBIDDEN {
            assert!(!code.contains(name), "doctor.rs names {name}");
            assert!(
                !glue.contains(name),
                "main.rs's no-prompt glue names {name}"
            );
        }
        // and the glue reaches Safari only behind --probe-safari's plan
        assert!(
            glue.contains("SafariRemoteAutomation"),
            "the probe is gated on the plan"
        );
    }

    /// The two reads the no-prompt path reaches stay reads: `setup.rs`'s `ax_trusted` and
    /// `screen_recording_ok` make exactly their preflight call and nothing else, and the glue
    /// calls exactly those two (fleet's SECAUDIT, advisory on #4).
    #[test]
    fn the_preflight_reads_stay_preflight() {
        let setup = include_str!("../../crates/mac/src/setup.rs");
        let body = |name: &str| -> String {
            let start = setup
                .find(&format!("pub fn {name}() -> bool {{"))
                .unwrap_or_else(|| panic!("setup.rs has no {name}"));
            let rest = &setup[start..];
            rest[..rest.find("\n}\n").expect("its body ends") + 2].to_string()
        };
        for (name, call) in [
            ("ax_trusted", "AXIsProcessTrusted()"),
            ("screen_recording_ok", "ScreenCaptureAccess.preflight()"),
        ] {
            let b = body(name);
            assert!(b.contains(call), "{name} no longer makes {call}: {b}");
            for bad in ["WithOptions", "request", "Prompt", "open", "Command"] {
                assert!(!b.contains(bad), "{name} names {bad}: {b}");
            }
            // one expression, the read: no statement before or after it
            assert_eq!(b.matches(';').count(), 0, "{name} does more than the read: {b}");
        }
        let main = include_str!("main.rs");
        let glue = main
            .split("// ── doctor --no-prompt ──")
            .nth(1)
            .expect("main.rs marks the no-prompt glue")
            .split("// ── end doctor --no-prompt ──")
            .next()
            .unwrap();
        for f in ["setup::ax_trusted()", "setup::screen_recording_ok()"] {
            assert!(glue.contains(f), "the no-prompt glue no longer reads through {f}");
        }
    }
}
