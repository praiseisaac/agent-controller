//! Line-delimited JSON IPC between the ephemeral CLI (client) and the long-lived
//! firefox daemon that owns the BiDi WebSocket. One request → one response per
//! connection.

use agent_controller_core::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Debug, Serialize, Deserialize)]
pub struct Req {
    pub op: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub n: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Resp {
    pub ok: bool,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Deterministic socket path for a firefox instance name.
pub fn socket_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push("agent-controller");
    let _ = std::fs::create_dir_all(&p);
    p.push(format!("firefox-{name}.sock"));
    p
}

/// Send one request to the daemon and read its response.
pub async fn request(socket: &Path, req: &Req) -> Result<Resp> {
    let mut stream = UnixStream::connect(socket)
        .await
        .map_err(|e| anyhow!("connecting to firefox daemon: {e}"))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    stream.shutdown().await.ok();
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await?;
    let resp: Resp = serde_json::from_str(buf.trim())
        .map_err(|e| anyhow!("bad daemon response {buf:?}: {e}"))?;
    Ok(resp)
}
