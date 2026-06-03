//! Ensure a `safaridriver` WebDriver server is running on a given port.

use agent_controller_core::{anyhow, Result};
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::Duration;

use crate::webdriver::WdClient;

/// Ensure safaridriver is serving on `port`; spawn it detached if not. Returns
/// the pid if we spawned it this call.
pub async fn ensure(port: u16) -> Result<Option<u32>> {
    let base = format!("http://127.0.0.1:{port}");
    if WdClient::ready(&base).await {
        return Ok(None);
    }
    let child = std::process::Command::new("safaridriver")
        .arg("-p")
        .arg(port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| anyhow!("spawning safaridriver: {e}"))?;
    let pid = child.id();
    drop(child);
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        if WdClient::ready(&base).await {
            return Ok(Some(pid));
        }
    }
    Err(anyhow!("safaridriver did not become ready on port {port}"))
}
