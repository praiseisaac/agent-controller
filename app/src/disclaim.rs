//! Running responsible for itself, so that the macOS grants agent-controller uses are its own.
//!
//! macOS judges Accessibility and Screen Recording on a process's *responsible* process,
//! not on the process itself. Started from a terminal, that is the terminal app. Started
//! by an orchestrator's session, it is whatever launched the session, and a grant there
//! covers every process the session runs, not only agent-controller. A build made with
//! `AGENT_CONTROLLER_DISCLAIM=1` in its environment (fleet's build sets it) runs the calls
//! that use those grants again, with responsibility disclaimed
//! (`responsibility_spawnattrs_setdisclaim`, as macOS's own helpers do). The grant is then
//! agent-controller's own, at its own path, and covers it and the children it starts.
//!
//! The loop guard is the state itself, not a variable a caller could set: a process that
//! is already responsible for itself runs. One that is not, and carries the marker, was
//! re-run and still is not, so it stops with an error rather than run on a grant that
//! is not its own. Only `DYLD_*` is dropped from the environment; agent-controller reads
//! `HOME` and its own settings, and starts browsers. The hardened runtime ignores `DYLD_*`
//! anyway. Stdio is inherited and the exit status passed back.

/// Whether this build runs its permission-using calls responsible for itself. Fixed when
/// the binary is built, never by the caller's environment at run time.
pub const BUILT_TO_DISCLAIM: bool = match option_env!("AGENT_CONTROLLER_DISCLAIM") {
    Some(v) => matches!(v.as_bytes(), b"1"),
    None => false,
};

/// Set in the re-run's environment: the loop guard.
pub const MARK: &str = "AGENT_CONTROLLER_DISCLAIMED";

/// Does this call use a grant macOS judges on the responsible process? The mac backend
/// (AX, events to a pid, screen capture), `--takeover` on any backend (the global
/// cursor), `doctor` (it reads both grants, and its plain form asks for them), and the MCP
/// server (any tool call may be the mac backend).
pub fn uses_grants(backend: &str, takeover: bool, doctor: bool, mcp: bool) -> bool {
    matches!(backend.to_ascii_lowercase().as_str(), "mac" | "macos") || takeover || doctor || mcp
}

/// What a call must do before running.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run here: this process is responsible for itself, or the call needs no grant.
    Run,
    /// Run this binary again, responsibility disclaimed, and pass back its exit status.
    Rerun,
    /// Refuse: re-run once already and still not responsible for itself.
    Refuse,
}

pub fn decide(wanted: bool, self_responsible: bool, marked: bool) -> Decision {
    match (wanted, self_responsible, marked) {
        (false, _, _) | (true, true, _) => Decision::Run,
        (true, false, false) => Decision::Rerun,
        (true, false, true) => Decision::Refuse,
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;

    extern "C" {
        fn responsibility_spawnattrs_setdisclaim(
            attr: *mut libc::posix_spawnattr_t,
            disclaim: libc::c_int,
        ) -> libc::c_int;
        fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
    }

    /// The process macOS holds responsible for `pid` (a read; it asks for nothing).
    pub fn responsible_for(pid: i32) -> i32 {
        unsafe { responsibility_get_pid_responsible_for_pid(pid) }
    }

    fn cstr(s: &OsStr) -> Option<CString> {
        CString::new(s.as_bytes()).ok()
    }

    /// Run this binary again with the same arguments, responsibility disclaimed, and
    /// return its exit status. Errors when the disclaim cannot be set: nothing runs then.
    pub fn rerun_disclaimed(mark: &str) -> anyhow::Result<i32> {
        let me = std::env::current_exe()?.canonicalize()?;
        let path = cstr(me.as_os_str()).ok_or_else(|| anyhow::anyhow!("own path has a NUL"))?;
        let mut args: Vec<CString> = vec![path.clone()];
        for a in std::env::args_os().skip(1) {
            args.push(cstr(&a).ok_or_else(|| anyhow::anyhow!("an argument has a NUL"))?);
        }
        let mut env: Vec<CString> = Vec::new();
        for (k, v) in std::env::vars_os() {
            if k.as_bytes().starts_with(b"DYLD_") || k.as_bytes() == mark.as_bytes() {
                continue;
            }
            let mut kv = k.as_bytes().to_vec();
            kv.push(b'=');
            kv.extend_from_slice(v.as_bytes());
            if let Ok(c) = CString::new(kv) {
                env.push(c);
            }
        }
        env.push(CString::new(format!("{mark}=1"))?);
        let mut argv: Vec<*mut libc::c_char> = args.iter().map(|c| c.as_ptr() as *mut _).collect();
        argv.push(std::ptr::null_mut());
        let mut envp: Vec<*mut libc::c_char> = env.iter().map(|c| c.as_ptr() as *mut _).collect();
        envp.push(std::ptr::null_mut());
        unsafe {
            let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
            if libc::posix_spawnattr_init(&mut attr) != 0 {
                anyhow::bail!("could not set up the run responsible for itself");
            }
            if responsibility_spawnattrs_setdisclaim(&mut attr, 1) != 0 {
                libc::posix_spawnattr_destroy(&mut attr);
                anyhow::bail!("could not run responsible for itself (the disclaim was refused); nothing was run");
            }
            let mut pid: libc::pid_t = 0;
            let rc = libc::posix_spawn(
                &mut pid,
                path.as_ptr(),
                std::ptr::null(),
                &attr,
                argv.as_ptr(),
                envp.as_ptr(),
            );
            libc::posix_spawnattr_destroy(&mut attr);
            if rc != 0 {
                anyhow::bail!("could not run itself responsible for itself (posix_spawn {rc})");
            }
            let mut status: libc::c_int = 0;
            while libc::waitpid(pid, &mut status, 0) < 0 {
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    anyhow::bail!("lost the run responsible for itself (waitpid)");
                }
            }
            Ok(if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                libc::WEXITSTATUS(status)
            })
        }
    }
}

/// Before a call runs: run it again responsible for itself when it needs that (`wanted`),
/// exiting with the re-run's status, or refuse. Returns only when this process should run
/// the call itself.
pub fn ensure_own_responsibility(wanted: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let me = std::process::id() as i32;
        match decide(wanted, sys::responsible_for(me) == me, std::env::var_os(MARK).is_some()) {
            Decision::Run => Ok(()),
            Decision::Rerun => std::process::exit(sys::rerun_disclaimed(MARK)?),
            Decision::Refuse => anyhow::bail!("ran again responsible for itself, and macOS still holds another process responsible; nothing was run"),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = wanted;
        Ok(())
    }
}

/// `__responsibility`: this process, the one macOS holds responsible for it, and whether
/// it was re-run. A read, for tests and for an orchestrator checking whose grant is used.
pub fn responsibility_record() -> serde_json::Value {
    let pid = std::process::id() as i32;
    #[cfg(target_os = "macos")]
    let responsible = sys::responsible_for(pid);
    #[cfg(not(target_os = "macos"))]
    let responsible = pid;
    serde_json::json!({
        "pid": pid,
        "responsible_pid": responsible,
        "self_responsible": responsible == pid,
        "rerun": std::env::var_os(MARK).is_some(),
        "built_to_disclaim": BUILT_TO_DISCLAIM,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calls_that_use_grants() {
        assert!(uses_grants("mac", false, false, false));
        assert!(uses_grants("MacOS", false, false, false));
        assert!(
            uses_grants("chrome", true, false, false),
            "--takeover is the global cursor"
        );
        assert!(
            uses_grants("ios-sim", false, true, false),
            "doctor reads both grants"
        );
        assert!(
            uses_grants("ios-sim", false, false, true),
            "the MCP may drive the mac backend"
        );
        for b in [
            "chrome",
            "firefox",
            "safari",
            "ios-sim",
            "android-emu",
            "windows",
        ] {
            assert!(!uses_grants(b, false, false, false), "{b}");
        }
    }

    #[test]
    fn a_marker_never_lets_a_call_run_on_another_process_grant() {
        assert_eq!(decide(true, false, false), Decision::Rerun);
        assert_eq!(decide(true, true, false), Decision::Run);
        assert_eq!(decide(true, true, true), Decision::Run);
        // the marker set by a caller, or a re-run that did not take: refused, never run
        assert_eq!(decide(true, false, true), Decision::Refuse);
        // a call that needs no grant runs wherever it is
        assert_eq!(decide(false, false, true), Decision::Run);
        assert_eq!(decide(false, false, false), Decision::Run);
    }
}
