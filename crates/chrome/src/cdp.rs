//! Minimal async Chrome DevTools Protocol client. One WebSocket multiplexes
//! many commands: each gets an auto-incrementing `id`; the reader matches
//! `{id,result}`/`{id,error}` responses back to the waiting caller. Event
//! messages (no `id`) are ignored.

use agent_controller_core::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

pub struct CdpClient {
    next_id: AtomicU64,
    cmd_tx: mpsc::UnboundedSender<Message>,
    pending: Pending,
}

impl CdpClient {
    pub async fn connect(url: &str) -> Result<Arc<Self>> {
        let (ws, _) = connect_async(url)
            .await
            .map_err(|e| anyhow!("connecting to CDP endpoint {url}: {e}"))?;
        let (mut write, mut read) = ws.split();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Message>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        tokio::spawn(async move {
            while let Some(msg) = cmd_rx.recv().await {
                if write.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let pending_r = pending.clone();
        tokio::spawn(async move {
            while let Some(Ok(msg)) = read.next().await {
                if let Message::Text(txt) = msg {
                    if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                        if let Some(id) = v.get("id").and_then(Value::as_u64) {
                            if let Some(tx) = pending_r.lock().await.remove(&id) {
                                if let Some(err) = v.get("error") {
                                    let msg = err
                                        .get("message")
                                        .and_then(Value::as_str)
                                        .unwrap_or("CDP error");
                                    let _ = tx.send(Err(msg.to_string()));
                                } else {
                                    let _ =
                                        tx.send(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
                                }
                            }
                        }
                    }
                }
            }
            let mut p = pending_r.lock().await;
            for (_, tx) in p.drain() {
                let _ = tx.send(Err("CDP connection closed".into()));
            }
        });

        Ok(Arc::new(Self {
            next_id: AtomicU64::new(1),
            cmd_tx,
            pending,
        }))
    }

    /// Send a CDP command and await its result.
    pub async fn send(&self, method: &str, params: Value) -> std::result::Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let envelope = json!({ "id": id, "method": method, "params": params });
        self.cmd_tx
            .send(Message::Text(envelope.to_string().into()))
            .map_err(|_| "CDP writer is gone".to_string())?;
        rx.await.map_err(|_| "CDP response dropped".to_string())?
    }
}
