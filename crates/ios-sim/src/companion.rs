//! Lifecycle for `idb_companion`: ensure a persistent gRPC server is running for
//! a given simulator UDID, reusing one across CLI invocations.

use agent_controller_core::{anyhow, Result};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::idb::companion_service_client::CompanionServiceClient;

/// Deterministic port in 11000..=11999 derived from the UDID (FNV-1a), so every
/// invocation targeting the same sim finds the same companion.
pub fn port_for(udid: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in udid.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    11000 + (h % 1000) as u16
}

async fn probe(endpoint: &str) -> bool {
    CompanionServiceClient::connect(endpoint.to_string())
        .await
        .is_ok()
}

fn spawn(udid: &str, port: u16, log: &std::path::Path) -> Result<u32> {
    let out = std::fs::File::create(log)?;
    let err = out.try_clone()?;
    let child = Command::new("idb_companion")
        .args([
            "--udid",
            udid,
            "--grpc-port",
            &port.to_string(),
            "--log-level",
            "info",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| {
            anyhow!(
                "failed to spawn idb_companion ({e}). Install with: \
                 brew install facebook/fb/idb-companion"
            )
        })?;
    Ok(child.id())
}

/// Ensure a companion is serving for `udid`; return its gRPC endpoint and, if we
/// spawned it this call, its pid. `log` is where a freshly-spawned companion logs.
pub async fn ensure(udid: &str, log: &std::path::Path) -> Result<(String, Option<u32>)> {
    let port = port_for(udid);
    let endpoint = format!("http://127.0.0.1:{port}");
    if probe(&endpoint).await {
        return Ok((endpoint, None));
    }
    let pid = spawn(udid, port, log)?;
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if probe(&endpoint).await {
            return Ok((endpoint, Some(pid)));
        }
    }
    Err(anyhow!(
        "idb_companion did not become reachable on {endpoint}; see {}",
        log.display()
    ))
}
