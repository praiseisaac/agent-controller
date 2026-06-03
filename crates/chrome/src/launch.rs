//! Launch Chrome with CDP enabled and discover a page target's WebSocket URL.

use agent_controller_core::{anyhow, Result};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

const DEFAULT_CHROME_PATHS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
];

pub fn find_chrome() -> Option<String> {
    if let Ok(p) = std::env::var("CHROME_BIN") {
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    DEFAULT_CHROME_PATHS
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
}

/// Launch Chrome with `--remote-debugging-port` and an isolated profile, then
/// wait until the CDP HTTP endpoint is up. Left running after this process exits.
pub async fn launch(port: u16, profile_dir: PathBuf) -> Result<Option<u32>> {
    let bin = find_chrome()
        .ok_or_else(|| anyhow!("Chrome not found. Set $CHROME_BIN or install Google Chrome."))?;
    std::fs::create_dir_all(&profile_dir).ok();

    let child = std::process::Command::new(&bin)
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--remote-allow-origins=*")
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Detach into its own process group so Chrome's many helper processes
        // don't keep the parent's pipe/process-group alive (and don't get
        // signalled when the launching CLI exits).
        .process_group(0)
        .spawn()
        .map_err(|e| anyhow!("spawning Chrome: {e}"))?;
    let pid = child.id();
    // Detach: don't wait, don't kill — Chrome persists across CLI invocations.
    drop(child);

    for _ in 0..100 {
        if reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
            .await
            .is_ok()
        {
            return Ok(Some(pid));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    Err(anyhow!("Chrome CDP endpoint did not come up on port {port}"))
}

/// Find (or create) a page target and return its `webSocketDebuggerUrl`.
pub async fn page_ws(port: u16) -> Result<String> {
    let list: Vec<serde_json::Value> = reqwest::get(format!("http://127.0.0.1:{port}/json"))
        .await
        .map_err(|e| anyhow!("listing CDP targets: {e}"))?
        .json()
        .await
        .map_err(|e| anyhow!("parsing CDP targets: {e}"))?;
    if let Some(ws) = list
        .iter()
        .find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
        .and_then(|t| t.get("webSocketDebuggerUrl"))
        .and_then(|v| v.as_str())
    {
        return Ok(ws.to_string());
    }
    // No page target: create one.
    let created: serde_json::Value =
        reqwest::get(format!("http://127.0.0.1:{port}/json/new?about:blank"))
            .await
            .map_err(|e| anyhow!("creating CDP target: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow!("parsing new target: {e}"))?;
    created
        .get("webSocketDebuggerUrl")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no page target available"))
}
