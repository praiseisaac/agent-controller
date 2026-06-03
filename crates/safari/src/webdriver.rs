//! Thin W3C WebDriver HTTP client for safaridriver.

use agent_controller_core::{anyhow, Result};
use serde_json::{json, Value};

pub struct WdClient {
    http: reqwest::Client,
    base: String,
    session: String,
}

/// Extract the `value` from a WebDriver response, mapping `{value:{error}}` to Err.
fn unwrap_value(v: Value) -> std::result::Result<Value, String> {
    let inner = v.get("value").cloned().unwrap_or(Value::Null);
    if let Some(err) = inner.get("error").and_then(|e| e.as_str()) {
        let msg = inner.get("message").and_then(|m| m.as_str()).unwrap_or(err);
        return Err(msg.to_string());
    }
    Ok(inner)
}

impl WdClient {
    /// Is the safaridriver server up and ready?
    pub async fn ready(base: &str) -> bool {
        let url = format!("{base}/status");
        match reqwest::get(&url).await {
            Ok(r) => r
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("value").and_then(|val| val.get("ready")).and_then(|b| b.as_bool()))
                .unwrap_or(true),
            Err(_) => false,
        }
    }

    /// Create a new Safari session.
    pub async fn new_session(base: &str) -> Result<Self> {
        let http = reqwest::Client::new();
        let resp: Value = http
            .post(format!("{base}/session"))
            .json(&json!({ "capabilities": { "alwaysMatch": { "browserName": "safari" } } }))
            .send()
            .await
            .map_err(|e| anyhow!("creating Safari session: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow!("parsing session response: {e}"))?;
        let value = unwrap_value(resp).map_err(|e| anyhow!("session not created: {e}"))?;
        let session = value
            .get("sessionId")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("no sessionId in response"))?
            .to_string();
        Ok(Self {
            http,
            base: base.to_string(),
            session,
        })
    }

    /// Reattach to an existing session id (validated by the caller).
    pub fn attach(base: &str, session: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.to_string(),
            session: session.to_string(),
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session
    }

    fn url(&self, path: &str) -> String {
        format!("{}/session/{}{}", self.base, self.session, path)
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        let resp: Value = self
            .http
            .post(self.url(path))
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("WebDriver POST {path}: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow!("parsing {path} response: {e}"))?;
        unwrap_value(resp).map_err(|e| anyhow!(e))
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp: Value = self
            .http
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| anyhow!("WebDriver GET {path}: {e}"))?
            .json()
            .await
            .map_err(|e| anyhow!("parsing {path} response: {e}"))?;
        unwrap_value(resp).map_err(|e| anyhow!(e))
    }

    /// Whether this session is still alive.
    pub async fn alive(&self) -> bool {
        self.get("/url").await.is_ok()
    }

    pub async fn navigate(&self, url: &str) -> Result<()> {
        self.post("/url", json!({ "url": url })).await.map(|_| ())
    }

    /// End the session (closes its Safari window).
    pub async fn delete_session(&self) -> Result<()> {
        self.http
            .delete(format!("{}/session/{}", self.base, self.session))
            .send()
            .await
            .map_err(|e| anyhow!("deleting session: {e}"))?;
        Ok(())
    }

    /// Run JS (the script body must `return` its result) and return the value.
    pub async fn execute(&self, script: &str) -> Result<Value> {
        self.post("/execute/sync", json!({ "script": script, "args": [] }))
            .await
    }

    pub async fn screenshot(&self) -> Result<String> {
        let v = self.get("/screenshot").await?;
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("screenshot returned no data"))
    }

    /// Send a key-input action sequence (Actions API).
    pub async fn key_actions(&self, actions: Value) -> Result<()> {
        self.post(
            "/actions",
            json!({ "actions": [{ "type": "key", "id": "kb", "actions": actions }] }),
        )
        .await
        .map(|_| ())
    }
}
