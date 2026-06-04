//! The firefox daemon: a long-lived process that launches/attaches Firefox,
//! owns the BiDi WebSocket + session for the browser's lifetime, and serves
//! action requests over a localhost TCP socket. CLI invocations are thin clients.

use crate::bidi::{BidiClient, BidiSession};
use crate::ensure;
use crate::ipc::{Req, Resp};
use agent_controller_core::{anyhow, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Run the daemon to completion (until a `close` request or socket error).
pub async fn run(name: String, profile_dir: PathBuf, addr: String) -> Result<()> {
    let (ws, _pid) = ensure(&name, profile_dir).await?;
    let client = BidiClient::connect(&format!("{ws}/session"))
        .await
        .map_err(|e| anyhow!("connecting BiDi: {e}"))?;
    let session = Arc::new(
        BidiSession::establish(client)
            .await
            .map_err(|e| anyhow!("establishing BiDi session: {e}"))?,
    );

    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow!("binding {addr}: {e}"))?;

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(c) => c,
            Err(_) => break,
        };
        let mut buf = String::new();
        if stream.read_to_string(&mut buf).await.is_err() {
            continue;
        }
        let req: Req = match serde_json::from_str(buf.trim()) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let close = req.op == "close";
        let resp = dispatch(&session, req).await;
        if let Ok(mut line) = serde_json::to_string(&resp) {
            line.push('\n');
            let _ = stream.write_all(line.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        if close {
            break;
        }
    }
    Ok(())
}

async fn dispatch(session: &Arc<BidiSession>, req: Req) -> Resp {
    let arg = |i: usize| req.args.get(i).cloned().unwrap_or_default();
    let result: std::result::Result<Option<serde_json::Value>, String> = match req.op.as_str() {
        "ping" => Ok(None),
        "navigate" => session.navigate(&arg(0)).await.map(|_| None),
        "eval" => session.evaluate(&arg(0)).await.map(Some),
        "type" => session.type_keys(&arg(0)).await.map(|_| None),
        "press" => session.press(&arg(0)).await.map(|_| None),
        "scroll" => session.scroll(&arg(0), req.n, None).await.map(|_| None),
        "screenshot" => session
            .screenshot()
            .await
            .map(|b64| Some(serde_json::Value::String(b64))),
        other => Err(format!("unknown op: {other}")),
    };
    match result {
        Ok(data) => Resp {
            ok: true,
            data,
            error: None,
        },
        Err(e) => Resp {
            ok: false,
            data: None,
            error: Some(e),
        },
    }
}
