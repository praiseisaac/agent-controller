//! Running responsible for itself, so that the macOS grants agent-controller uses are its own.
//!
//! macOS judges Accessibility and Screen Recording on a process's *responsible* process,
//! not on the process itself. Started from a terminal, that is the terminal app. Started
//! by an orchestrator's session, it is whatever launched the session, and a grant there
//! covers every process the session runs, not only agent-controller. A build made with
//! `AGENT_CONTROLLER_DISCLAIM=1` in its environment (fleet's build sets it) runs every call
//! again, with responsibility disclaimed (`responsibility_spawnattrs_setdisclaim`, as
//! macOS's own helpers do), but a closed list that never touches a grant (`main.rs`
//! `runs_in_place`: a call added later disclaims unless it is added there). The grant is
//! then agent-controller's own, at its own path, and covers it and the children it starts.
//!
//! The rules (fleet's SECAUDIT AC12):
//! - A call that has not been re-run is ALWAYS re-run. Reading as responsible for itself is
//!   not enough: a process whose responsible process has exited reads that way, while macOS
//!   still judges it on the exited process's identity. Nothing runs in place on that read.
//! - The re-run carries the marker, set to the pid of the process that spawned it. A child
//!   accepts it only when that pid is its parent's and its parent is this same binary, and
//!   it is responsible for itself. A marker any other process set is refused.
//! - The marker is taken out of this process's environment before the call runs, so
//!   nothing the call starts inherits it.
//! - A spawn that fails refuses: never a fallback to running in place.
//!
//! Only `DYLD_*` is dropped from the re-run's environment; agent-controller reads `HOME` and
//! its own settings, and starts browsers. The hardened runtime ignores `DYLD_*` anyway. Stdio
//! is inherited and the exit status passed back.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Whether this build runs its permission-using calls responsible for itself. Fixed when
/// the binary is built, never by the caller's environment at run time.
pub const BUILT_TO_DISCLAIM: bool = match option_env!("AGENT_CONTROLLER_DISCLAIM") {
    Some(v) => matches!(v.as_bytes(), b"1"),
    None => false,
};

/// Set in the re-run's environment, to the spawning process's pid: the loop guard.
pub const MARK: &str = "AGENT_CONTROLLER_DISCLAIMED";

/// The marker this process started with, as it was judged (taken out of its environment).
static STARTED_WITH: OnceLock<Marker> = OnceLock::new();

/// The marker a process started with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// none: not a re-run
    None,
    /// set by this binary's own disclaimed spawn: its parent, by pid, and the same binary
    Ours,
    /// set by anything else: a caller, or a parent that is not this binary
    Foreign,
}

/// Judge a marker's value against this process's parent: ours only when it names the
/// parent's pid and the parent runs this same binary.
pub fn classify(
    value: Option<&str>,
    parent_pid: i32,
    parent_exe: Option<&Path>,
    own_exe: Option<&Path>,
) -> Marker {
    let Some(v) = value else { return Marker::None };
    match (v.parse::<i32>(), parent_exe, own_exe) {
        (Ok(pid), Some(p), Some(me)) if pid == parent_pid && pid > 1 && p == me => Marker::Ours,
        _ => Marker::Foreign,
    }
}

/// What a call must do before running.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run here: the call needs no grant, or this is our own re-run and responsible for itself.
    Run,
    /// Run this binary again, responsibility disclaimed, and pass back its exit status.
    Rerun,
    /// Refuse: a marker this binary did not set, or a re-run still not responsible for itself.
    Refuse,
}

pub fn decide(wanted: bool, self_responsible: bool, marker: Marker) -> Decision {
    match (wanted, marker, self_responsible) {
        (false, _, _) => Decision::Run,
        // not re-run yet: re-run, whatever the read says (AC12a)
        (true, Marker::None, _) => Decision::Rerun,
        (true, Marker::Ours, true) => Decision::Run,
        (true, Marker::Ours, false) | (true, Marker::Foreign, _) => Decision::Refuse,
    }
}

/// What to do with a decision: `Ok(None)` runs the call here, `Ok(Some(status))` exits with
/// the re-run's status, an error runs nothing. A re-run that cannot be spawned is an error,
/// never a run in place (AC12c).
pub fn act(
    decision: Decision,
    rerun: impl FnOnce() -> anyhow::Result<i32>,
) -> anyhow::Result<Option<i32>> {
    match decision {
        Decision::Run => Ok(None),
        Decision::Rerun => Ok(Some(rerun()?)),
        Decision::Refuse => anyhow::bail!(
            "it carries a marker its own disclaimed run did not set, or ran again and macOS still holds another process responsible; nothing was run"
        ),
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

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

    /// The executable a process runs, resolved (a read of the process table).
    pub fn exe_of(pid: i32) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe {
            libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut libc::c_void, buf.len() as u32)
        };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        PathBuf::from(OsStr::from_bytes(&buf)).canonicalize().ok()
    }

    fn cstr(s: &OsStr) -> Option<CString> {
        CString::new(s.as_bytes()).ok()
    }

    /// Run this binary again with the same arguments, responsibility disclaimed, the marker
    /// set to this process's pid, and return its exit status. Errors when the disclaim
    /// cannot be set or the spawn fails: nothing runs then.
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
        env.push(CString::new(format!("{mark}={}", std::process::id()))?);
        let mut argv: Vec<*mut libc::c_char> = args.iter().map(|c| c.as_ptr() as *mut _).collect();
        argv.push(std::ptr::null_mut());
        let mut envp: Vec<*mut libc::c_char> = env.iter().map(|c| c.as_ptr() as *mut _).collect();
        envp.push(std::ptr::null_mut());
        unsafe {
            let mut attr: libc::posix_spawnattr_t = std::ptr::null_mut();
            if libc::posix_spawnattr_init(&mut attr) != 0 {
                anyhow::bail!("could not set up the run responsible for itself; nothing was run");
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
                anyhow::bail!("could not run itself responsible for itself (posix_spawn {rc}); nothing was run");
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

/// The marker this process started with, judged against its parent, and taken out of its
/// environment so that nothing it starts inherits it (AC12b). Judged once. `main` calls it
/// first, while it is the only thread: the environment is changed before any runtime
/// thread could read it (fleet's SECAUDIT advisory on #7). Later calls read the judgement.
pub fn take_marker() -> Marker {
    *STARTED_WITH.get_or_init(|| {
        let value = std::env::var(MARK).ok();
        std::env::remove_var(MARK);
        #[cfg(target_os = "macos")]
        {
            let parent = std::os::unix::process::parent_id() as i32;
            let parent_exe: Option<PathBuf> = sys::exe_of(parent);
            let own_exe: Option<PathBuf> = std::env::current_exe()
                .ok()
                .and_then(|p| p.canonicalize().ok());
            classify(
                value.as_deref(),
                parent,
                parent_exe.as_deref(),
                own_exe.as_deref(),
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            if value.is_some() {
                Marker::Foreign
            } else {
                Marker::None
            }
        }
    })
}

/// Before a call runs: run it again responsible for itself when it needs that (`wanted`),
/// exiting with the re-run's status, or refuse. Returns only when this process should run
/// the call itself. The marker is taken out of the environment whatever is decided.
pub fn ensure_own_responsibility(wanted: bool) -> anyhow::Result<()> {
    let marker = take_marker();
    #[cfg(target_os = "macos")]
    {
        let me = std::process::id() as i32;
        let decision = decide(wanted, sys::responsible_for(me) == me, marker);
        if let Some(status) = act(decision, || sys::rerun_disclaimed(MARK))? {
            std::process::exit(status);
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (wanted, marker);
        Ok(())
    }
}

/// `__responsibility`: this process, the one macOS holds responsible for it, whether it is
/// its own disclaimed re-run, and whether the marker is still in its environment (it never
/// is: nothing it starts may inherit it). A read, for tests and for an orchestrator.
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
        "rerun": take_marker() == Marker::Ours,
        "marker_in_env": std::env::var_os(MARK).is_some(),
        "built_to_disclaim": BUILT_TO_DISCLAIM,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unmarked_call_always_runs_again_even_when_it_reads_as_its_own() {
        assert_eq!(decide(true, false, Marker::None), Decision::Rerun);
        // AC12a: an orphan (its responsible process exited) reads as responsible for itself
        // and is still judged on the exited process: it re-runs too
        assert_eq!(decide(true, true, Marker::None), Decision::Rerun);
        // a call that needs no grant runs wherever it is, marker or not
        for m in [Marker::None, Marker::Ours, Marker::Foreign] {
            assert_eq!(decide(false, false, m), Decision::Run);
            assert_eq!(decide(false, true, m), Decision::Run);
        }
    }

    #[test]
    fn only_our_own_re_run_runs_and_only_responsible_for_itself() {
        assert_eq!(decide(true, true, Marker::Ours), Decision::Run);
        assert_eq!(decide(true, false, Marker::Ours), Decision::Refuse);
        // AC12b: a marker another process set is refused, whatever the read says
        assert_eq!(decide(true, true, Marker::Foreign), Decision::Refuse);
        assert_eq!(decide(true, false, Marker::Foreign), Decision::Refuse);
    }

    #[test]
    fn a_marker_is_ours_only_from_this_binary_as_the_parent_it_names() {
        let me = Path::new("/opt/x/agent-controller");
        let other = Path::new("/bin/zsh");
        assert_eq!(classify(None, 40, Some(me), Some(me)), Marker::None);
        assert_eq!(classify(Some("40"), 40, Some(me), Some(me)), Marker::Ours);
        // another pid than the parent's; a parent that is another program; a value that is
        // not a pid; launchd; a parent whose program cannot be read
        assert_eq!(
            classify(Some("41"), 40, Some(me), Some(me)),
            Marker::Foreign
        );
        assert_eq!(
            classify(Some("40"), 40, Some(other), Some(me)),
            Marker::Foreign
        );
        assert_eq!(classify(Some("1"), 1, Some(me), Some(me)), Marker::Foreign);
        assert_eq!(
            classify(Some("yes"), 40, Some(me), Some(me)),
            Marker::Foreign
        );
        assert_eq!(classify(Some(""), 40, Some(me), Some(me)), Marker::Foreign);
        assert_eq!(classify(Some("40"), 40, None, Some(me)), Marker::Foreign);
    }

    #[test]
    fn a_spawn_that_fails_refuses_and_never_runs_in_place() {
        // AC12c
        let failed = act(Decision::Rerun, || anyhow::bail!("posix_spawn 1"));
        assert!(failed.is_err(), "{failed:?}");
        assert_eq!(act(Decision::Rerun, || Ok(3)).unwrap(), Some(3));
        let mut spawned = false;
        assert!(act(Decision::Refuse, || {
            spawned = true;
            Ok(0)
        })
        .is_err());
        assert!(!spawned, "a refusal spawns nothing");
        assert_eq!(
            act(Decision::Run, || anyhow::bail!("not asked")).unwrap(),
            None
        );
    }

    /// The marker leaves the environment while main is the only thread: main calls
    /// take_marker() before it builds any runtime, and no #[tokio::main] starts one first
    /// (fleet's SECAUDIT advisory on #7).
    #[test]
    fn main_takes_the_marker_before_any_runtime_thread_starts() {
        let main = include_str!("main.rs");
        assert!(
            !main.contains("#[tokio::main]"),
            "a #[tokio::main] starts its threads before main's first line"
        );
        let body = &main[main.find("\nfn main() {").expect("a synchronous fn main")..];
        let take = body
            .find("disclaim::take_marker();")
            .expect("main takes the marker");
        let runtime = body
            .find("tokio::runtime::")
            .expect("main builds the runtime");
        assert!(
            take < runtime,
            "main takes the marker before it builds the runtime"
        );
    }
}
