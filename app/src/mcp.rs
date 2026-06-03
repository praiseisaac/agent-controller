//! MCP server (stdio, JSON-RPC 2.0) exposing the backend-agnostic `Controller`
//! verbs as tools. Each `tools/call` builds a controller via the same
//! `factory::create` path the CLI uses, so every backend (mac/ios-sim/firefox/
//! chrome/safari) is drivable by an MCP client (e.g. Claude) with no per-backend
//! code here.

use crate::factory;
use agent_controller_core::{Backend, Locator, Options, ScrollDir, SessionStore};
use anyhow::Result;
use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL_VERSION: &str = "2024-11-05";

pub async fn serve() -> Result<()> {
    let store = SessionStore::discover(None)?;
    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = stdin.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) get no response.
        let response = match method {
            "initialize" => Some(ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "agent-controller", "version": env!("CARGO_PKG_VERSION") }
                }),
            )),
            "tools/list" => Some(ok(id, json!({ "tools": tool_specs() }))),
            "tools/call" => Some(ok(id, call_tool(&store, params).await)),
            "ping" => Some(ok(id, json!({}))),
            _ if id.is_some() => Some(err(id, -32601, &format!("method not found: {method}"))),
            _ => None, // notification
        };

        if let Some(resp) = response {
            let mut s = serde_json::to_string(&resp)?;
            s.push('\n');
            stdout.write_all(s.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

fn ok(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Common target-selection properties shared by every tool.
fn target_props() -> Value {
    json!({
        "backend": { "type": "string", "enum": ["mac","ios-sim","firefox","chrome","safari"],
                     "description": "Which backend to drive." },
        "session": { "type": "string", "description": "Instance name for browsers (default 'default')." },
        "udid": { "type": "string", "description": "iOS simulator UDID (ios-sim; default booted)." },
        "app": { "type": "string", "description": "App bundle id or name (mac)." }
    })
}

fn merge(base: Value, extra: Vec<(&str, Value)>) -> Value {
    let mut obj = base.as_object().cloned().unwrap_or_default();
    for (k, v) in extra {
        obj.insert(k.to_string(), v);
    }
    Value::Object(obj)
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": desc,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": required.iter().map(|s| json!(s)).collect::<Vec<_>>()
        }
    })
}

fn tool_specs() -> Vec<Value> {
    let p = target_props();
    vec![
        tool(
            "navigate",
            "Open a target: a URL (browsers) or an app/bundle-id (mac/ios-sim), or 'home'.",
            merge(p.clone(), vec![("target", json!({ "type": "string" }))]),
            &["backend", "target"],
        ),
        tool(
            "snapshot",
            "Return the accessibility/DOM tree with @ref ids for the current screen/page.",
            p.clone(),
            &["backend"],
        ),
        tool(
            "click",
            "Click/tap an element. Locator: @ref, \"Label text\", css:#sel, or (x,y).",
            merge(p.clone(), vec![("locator", json!({ "type": "string" }))]),
            &["backend", "locator"],
        ),
        tool(
            "type",
            "Type text into the focused element.",
            merge(p.clone(), vec![("text", json!({ "type": "string" }))]),
            &["backend", "text"],
        ),
        tool(
            "press",
            "Press a key or chord, e.g. Enter, Tab, Cmd+A.",
            merge(p.clone(), vec![("key", json!({ "type": "string" }))]),
            &["backend", "key"],
        ),
        tool(
            "scroll",
            "Scroll up/down/left/right by an amount in points.",
            merge(
                p.clone(),
                vec![
                    ("direction", json!({ "type": "string", "enum": ["up","down","left","right"] })),
                    ("amount", json!({ "type": "integer", "default": 400 })),
                ],
            ),
            &["backend", "direction"],
        ),
        tool(
            "screenshot",
            "Capture a screenshot of the target as a PNG image.",
            p.clone(),
            &["backend"],
        ),
    ]
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn error_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

async fn call_tool(store: &SessionStore, params: Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match run_tool(store, name, &args).await {
        Ok(v) => v,
        Err(e) => error_result(format!("{e:#}")),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

async fn run_tool(store: &SessionStore, name: &str, args: &Value) -> Result<Value> {
    let backend: Backend = str_arg(args, "backend").unwrap_or("ios-sim").parse()?;
    let opts = Options {
        udid: str_arg(args, "udid").map(str::to_string),
        app: str_arg(args, "app").map(str::to_string),
        session: str_arg(args, "session").map(str::to_string),
        takeover: args.get("takeover").and_then(|v| v.as_bool()).unwrap_or(false),
    };
    let (ctrl, _id) = factory::create(store, backend, opts).await?;

    Ok(match name {
        "navigate" => {
            let target = str_arg(args, "target").unwrap_or_default();
            ctrl.navigate(target).await?;
            text_result(format!("opened {target}"))
        }
        "snapshot" => {
            let snap = ctrl.snapshot().await?;
            text_result(snap.render())
        }
        "click" => {
            let loc = Locator::parse(str_arg(args, "locator").unwrap_or_default());
            ctrl.click(&loc).await?;
            text_result("clicked".into())
        }
        "type" => {
            let t = str_arg(args, "text").unwrap_or_default();
            ctrl.type_text(t).await?;
            text_result(format!("typed {} chars", t.chars().count()))
        }
        "press" => {
            let k = str_arg(args, "key").unwrap_or_default();
            ctrl.press(k).await?;
            text_result(format!("pressed {k}"))
        }
        "scroll" => {
            let dir: ScrollDir = str_arg(args, "direction").unwrap_or("down").parse()?;
            let amount = args.get("amount").and_then(|v| v.as_i64()).unwrap_or(400) as i32;
            ctrl.scroll(dir, amount).await?;
            text_result("scrolled".into())
        }
        "screenshot" => {
            let img = ctrl.screenshot().await?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&img.data);
            json!({ "content": [{ "type": "image", "data": b64, "mimeType": "image/png" }] })
        }
        other => return Err(anyhow::anyhow!("unknown tool: {other}")),
    })
}
