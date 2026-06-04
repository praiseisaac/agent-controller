#![cfg(target_os = "macos")]
//! Safari backend for `agent-controller`, driving Safari via `safaridriver`
//! (W3C WebDriver over HTTP). safaridriver is the persistent server; we keep it
//! running and reuse a stored `sessionId` across CLI invocations. Instances are
//! keyed by `--session <name>` (default `default`) → its own safaridriver port +
//! WebDriver session.
//!
//! Requires "Allow Remote Automation" in Safari (Develop menu) — see
//! `agent-controller doctor`.

mod launch;
mod webdriver;

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image, Locator,
    Options, Rect, Result, ScrollDir, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use webdriver::WdClient;

fn port_for(name: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    9500 + (h % 100) as u16
}

fn js_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// WebDriver key codepoints (subset).
fn modifier_codepoint(name: &str) -> Option<&'static str> {
    Some(match name {
        "shift" => "\u{E008}",
        "ctrl" | "control" => "\u{E009}",
        "alt" | "option" | "opt" => "\u{E00A}",
        "cmd" | "command" | "meta" | "super" => "\u{E03D}",
        _ => return None,
    })
}

fn base_key_value(name: &str) -> Result<String> {
    let v = match name {
        "enter" | "return" => "\u{E007}",
        "tab" => "\u{E004}",
        "escape" | "esc" => "\u{E00C}",
        "backspace" => "\u{E003}",
        "delete" => "\u{E017}",
        "space" => " ",
        "up" => "\u{E013}",
        "down" => "\u{E015}",
        "left" => "\u{E012}",
        "right" => "\u{E014}",
        s if s.chars().count() == 1 => return Ok(s.to_string()),
        other => return Err(anyhow!("unknown key: {other}")),
    };
    Ok(v.to_string())
}

pub struct SafariController {
    wd: WdClient,
    target: String,
}

#[derive(Deserialize)]
struct RawEl {
    r#ref: String,
    role: Option<String>,
    name: Option<String>,
    value: Option<String>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[async_trait]
impl Controller for SafariController {
    async fn navigate(&self, target: &str) -> Result<()> {
        let url = if target.contains("://") || target.starts_with("about:") {
            target.to_string()
        } else {
            format!("https://{target}")
        };
        self.wd.navigate(&url).await
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let js = r##"return (() => {
  document.querySelectorAll('[data-abf-ref]').forEach(e => e.removeAttribute('data-abf-ref'));
  const sels = 'a,button,input,select,textarea,[role],[onclick],summary,[contenteditable="true"],h1,h2,h3';
  const visible = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width>0 && r.height>0 && s.visibility!=='hidden' && s.display!=='none'; };
  const out = []; let n = 0;
  document.querySelectorAll(sels).forEach(el => {
    if (!visible(el)) return; n++; const ref = 'e'+n; el.setAttribute('data-abf-ref', ref);
    const r = el.getBoundingClientRect();
    const role = el.getAttribute('role') || el.tagName.toLowerCase();
    let name = (el.getAttribute('aria-label')||el.getAttribute('placeholder')||el.value||el.innerText||el.getAttribute('alt')||el.getAttribute('title')||'').trim().replace(/\s+/g,' ').slice(0,80);
    out.push({ ref, role, name, value: (el.value||''), x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) });
  });
  return out;
})();"##;
        let v = self.wd.execute(js).await?;
        let raws: Vec<RawEl> = serde_json::from_value(v).unwrap_or_default();
        let elements = raws
            .into_iter()
            .map(|r| Element {
                r#ref: r.r#ref,
                role: r.role,
                label: r.name.filter(|s| !s.is_empty()),
                value: r.value.filter(|s| !s.is_empty()),
                frame: Rect {
                    x: r.x,
                    y: r.y,
                    width: r.w,
                    height: r.h,
                },
                enabled: true,
                actions: Vec::new(),
            })
            .collect();
        Ok(Snapshot { elements })
    }

    async fn click(&self, loc: &Locator) -> Result<()> {
        let js = match loc {
            Locator::Ref(r) => format!(
                "return (() => {{ const el=document.querySelector('[data-abf-ref=\"{r}\"]'); if(!el) throw new Error('no element @{r}'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})();"
            ),
            Locator::Css(s) => format!(
                "return (() => {{ const el=document.querySelector({sel}); if(!el) throw new Error('no element'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})();",
                sel = js_str(s)
            ),
            Locator::Label(s) | Locator::Text(s) => format!(
                "return (() => {{ const t={needle}.toLowerCase(); const el=[...document.querySelectorAll('a,button,input,select,textarea,[role],[onclick],summary')].find(e => ((e.innerText||e.value||e.getAttribute('aria-label')||'').trim().toLowerCase().includes(t)) && e.getBoundingClientRect().width>0); if(!el) throw new Error('no element with text'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})();",
                needle = js_str(&s.to_lowercase())
            ),
            Locator::Role { role, name } => format!(
                "return (() => {{ const role={r}; const nm={n}; const el=[...document.querySelectorAll('[role],a,button,input,select,textarea')].find(e => ((e.getAttribute('role')||e.tagName.toLowerCase())===role) && (!nm || ((e.innerText||e.value||e.getAttribute('aria-label')||'').toLowerCase().includes(nm)))); if(!el) throw new Error('no matching role'); el.scrollIntoView({{block:'center'}}); el.click(); return true; }})();",
                r = js_str(role),
                n = js_str(&name.clone().unwrap_or_default().to_lowercase())
            ),
            Locator::Point { x, y } => format!(
                "return (() => {{ const el=document.elementFromPoint({x},{y}); if(!el) throw new Error('no element at point'); el.click(); return true; }})();"
            ),
        };
        self.wd.execute(&js).await.map(|_| ())
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        let mut actions = Vec::new();
        for ch in text.chars() {
            let s = ch.to_string();
            actions.push(json!({ "type": "keyDown", "value": s }));
            actions.push(json!({ "type": "keyUp", "value": s }));
        }
        self.wd.key_actions(Value::Array(actions)).await
    }

    async fn press(&self, key: &str) -> Result<()> {
        let mut mods: Vec<String> = Vec::new();
        let mut base: Option<String> = None;
        for part in key.split('+') {
            let p = part.trim().to_ascii_lowercase();
            if let Some(m) = modifier_codepoint(&p) {
                mods.push(m.to_string());
            } else {
                base = Some(p);
            }
        }
        let base = base.ok_or_else(|| anyhow!("no base key in {key:?}"))?;
        let bval = base_key_value(&base)?;
        let mut actions = Vec::new();
        for m in &mods {
            actions.push(json!({ "type": "keyDown", "value": m }));
        }
        actions.push(json!({ "type": "keyDown", "value": bval }));
        actions.push(json!({ "type": "keyUp", "value": bval }));
        for m in mods.iter().rev() {
            actions.push(json!({ "type": "keyUp", "value": m }));
        }
        self.wd.key_actions(Value::Array(actions)).await
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let a = amount.max(1);
        let (dx, dy) = match dir {
            ScrollDir::Up => (0, -a),
            ScrollDir::Down => (0, a),
            ScrollDir::Left => (-a, 0),
            ScrollDir::Right => (a, 0),
        };
        self.wd
            .execute(&format!("return (() => {{ window.scrollBy({dx}, {dy}); return true; }})();"))
            .await
            .map(|_| ())
    }

    async fn screenshot(&self) -> Result<Image> {
        let b64 = self.wd.screenshot().await?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| anyhow!("decoding screenshot: {e}"))?;
        Ok(Image {
            data,
            format: "png".into(),
        })
    }

    fn backend(&self) -> Backend {
        Backend::Safari
    }

    fn target(&self) -> String {
        self.target.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            eval: true,
            network: false,
            menus: false,
            coordinates: true,
            screenshot: true,
        }
    }
}

/// Check whether Safari's "Allow Remote Automation" is enabled, by attempting a
/// throwaway WebDriver session (briefly opens + closes a Safari window). Returns
/// `(safaridriver_present, remote_automation_enabled)`.
pub async fn remote_automation_status() -> (bool, bool) {
    let port = 9599; // dedicated probe port
    if launch::ensure(port).await.is_err() {
        return (false, false);
    }
    match WdClient::new_session(&format!("http://127.0.0.1:{port}")).await {
        Ok(wd) => {
            let _ = wd.delete_session().await;
            (true, true)
        }
        Err(e) => {
            // Only the specific "Allow Remote Automation" error means it's off;
            // other errors (e.g. a session already active) imply it IS enabled.
            let msg = e.to_string().to_lowercase();
            let disabled = msg.contains("remote automation") || msg.contains("developer");
            (true, !disabled)
        }
    }
}

/// Session-aware entry point for the Safari backend.
pub struct SafariFactory;

#[async_trait]
impl BackendFactory for SafariFactory {
    fn backend(&self) -> Backend {
        Backend::Safari
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let name = opts.session.clone().unwrap_or_else(|| "default".to_string());
        Ok(Identity {
            id: format!("safari/{name}"),
            target: name,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        _store: &SessionStore,
        _opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        let name = rec.target.clone();
        let port = port_for(&name);
        let base = format!("http://127.0.0.1:{port}");
        let pid = launch::ensure(port).await?;

        // Reuse a stored, still-alive WebDriver session; else create one.
        let stored = rec.config.get("session_id").and_then(|v| v.as_str()).map(str::to_string);
        let wd = match stored {
            Some(sid) => {
                let candidate = WdClient::attach(&base, &sid);
                if candidate.alive().await {
                    candidate
                } else {
                    WdClient::new_session(&base).await?
                }
            }
            None => WdClient::new_session(&base).await?,
        };

        rec.runtime.endpoint = Some(base);
        rec.runtime.alive = true;
        if let Some(p) = pid {
            rec.runtime.pids = vec![p];
        }
        rec.config = json!({ "port": port, "session_id": wd.session_id() });
        Ok(Box::new(SafariController { wd, target: name }))
    }
}
