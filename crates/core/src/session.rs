//! Persistent, per-item session store. A session is keyed by `<backend>/<instance>`
//! (e.g. `ios-sim/<udid>`, `chrome/work`) and owns its config, the live backend
//! process handle, the `@ref` cache, and artifacts (screenshots, files, logs).

use crate::{anyhow, Backend, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How to re-attach to a backend's live process.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Runtime {
    /// gRPC endpoint / daemon socket, if running.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Processes this tool manages (companion / daemon).
    #[serde(default)]
    pub pids: Vec<u32>,
    /// Whether the process was reachable at last contact.
    #[serde(default)]
    pub alive: bool,
}

/// The on-disk source of truth for one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub backend: Backend,
    /// udid / url / bundle-id / app.
    pub target: String,
    pub created_at: u64,
    pub last_used_at: u64,
    #[serde(default)]
    pub runtime: Runtime,
    /// Backend-specific config; opaque to core.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Breadcrumbs (last screen, snapshot time, ...).
    #[serde(default)]
    pub last: serde_json::Value,
}

impl SessionRecord {
    pub fn new(id: impl Into<String>, backend: Backend, target: impl Into<String>) -> Self {
        let t = now();
        SessionRecord {
            id: id.into(),
            backend,
            target: target.into(),
            created_at: t,
            last_used_at: t,
            runtime: Runtime::default(),
            config: serde_json::Value::Null,
            last: serde_json::Value::Null,
        }
    }
    pub fn touch(&mut self) {
        self.last_used_at = now();
    }
}

/// Per-session paths handed to backends. Directories are created on demand.
#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub dir: PathBuf,
    pub refs: PathBuf,
    pub screenshots: PathBuf,
    pub files: PathBuf,
    pub log: PathBuf,
}

/// The session store rooted at a `.agent-controller` home directory.
pub struct SessionStore {
    home: PathBuf,
}

impl SessionStore {
    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        SessionStore { home: home.into() }
    }

    /// Resolve the store location: `AGENT_CONTROLLER_HOME` → a project-local
    /// `.agent-controller/` found by walking up from cwd → `~/.agent-controller/`.
    pub fn discover(home_override: Option<PathBuf>) -> Result<Self> {
        if let Some(h) = home_override {
            return Ok(Self::with_home(h));
        }
        if let Ok(h) = std::env::var("AGENT_CONTROLLER_HOME") {
            return Ok(Self::with_home(h));
        }
        if let Ok(mut dir) = std::env::current_dir() {
            loop {
                let cand = dir.join(".agent-controller");
                if cand.is_dir() {
                    return Ok(Self::with_home(cand));
                }
                if !dir.pop() {
                    break;
                }
            }
        }
        let home = dirs::home_dir()
            .ok_or_else(|| anyhow!("cannot determine home directory"))?
            .join(".agent-controller");
        Ok(Self::with_home(home))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn session_dir(&self, id: &str) -> PathBuf {
        // id is "<backend>/<key>"; nested dirs fall out naturally.
        self.home.join("sessions").join(id)
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.session_dir(id).join("session.json")
    }

    /// Artifact/cache paths for a session (creates the directories).
    pub fn paths(&self, id: &str) -> Result<SessionPaths> {
        let dir = self.session_dir(id);
        let screenshots = dir.join("screenshots");
        let files = dir.join("files");
        std::fs::create_dir_all(&screenshots)?;
        std::fs::create_dir_all(&files)?;
        Ok(SessionPaths {
            refs: dir.join("refs.json"),
            log: dir.join("companion.log"),
            screenshots,
            files,
            dir,
        })
    }

    pub fn load(&self, id: &str) -> Result<Option<SessionRecord>> {
        let path = self.record_path(id);
        match std::fs::read_to_string(&path) {
            Ok(s) => Ok(Some(serde_json::from_str(&s)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, rec: &SessionRecord) -> Result<()> {
        let path = self.record_path(&rec.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(rec)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let dir = self.session_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// All sessions, walking `sessions/<backend>/<key>/session.json`.
    pub fn list(&self) -> Result<Vec<SessionRecord>> {
        let root = self.home.join("sessions");
        let mut out = Vec::new();
        let Ok(backends) = std::fs::read_dir(&root) else {
            return Ok(out);
        };
        for backend in backends.flatten() {
            if !backend.path().is_dir() {
                continue;
            }
            let Ok(keys) = std::fs::read_dir(backend.path()) else {
                continue;
            };
            for key in keys.flatten() {
                let rp = key.path().join("session.json");
                if let Ok(s) = std::fs::read_to_string(&rp) {
                    if let Ok(rec) = serde_json::from_str::<SessionRecord>(&s) {
                        out.push(rec);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.last_used_at.cmp(&a.last_used_at));
        Ok(out)
    }
}
