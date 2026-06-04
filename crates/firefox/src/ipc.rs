//! Line-delimited JSON IPC between the ephemeral CLI (client) and the long-lived
//! firefox daemon that owns the BiDi WebSocket. One request → one response per
//! connection, over a localhost TCP socket (cross-platform).

use agent_controller_core::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

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

/// Deterministic loopback address for a firefox instance's daemon (port
/// 9700..=9799 from the name — distinct from the BiDi debug port).
pub fn daemon_addr(name: &str) -> String {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    format!("127.0.0.1:{}", 9700 + (h % 100) as u16)
}

/// Send one request to the daemon and read its response.
pub async fn request(addr: &str, req: &Req) -> Result<Resp> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| anyhow!("connecting to firefox daemon at {addr}: {e}"))?;
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    stream.shutdown().await.ok(); // half-close so the daemon reads to EOF
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await?;
    serde_json::from_str(buf.trim()).map_err(|e| anyhow!("bad daemon response {buf:?}: {e}"))
}
