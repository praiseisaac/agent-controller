//! Chrome backend for `agent-controller`, driving Chrome over the DevTools
//! Protocol (CDP). Unlike Firefox's BiDi, CDP permits transient connections, so
//! each CLI invocation reconnects to a persistent Chrome — no daemon needed.
//! Instances are keyed by `--session <name>` (default `default`) → an isolated
//! profile + debug port.

mod cdp;
mod launch;

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image, Locator,
    Options, Rect, Result, ScrollDir, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use base64::Engine;
use cdp::CdpClient;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn port_for(name: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    9400 + (h % 100) as u16
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

pub struct ChromeController {
    cdp: Arc<CdpClient>,
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

impl ChromeController {
    async fn eval(&self, js: &str) -> Result<serde_json::Value> {
        let r = self
            .cdp
            .send(
                "Runtime.evaluate",
                json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
            )
            .await
            .map_err(|e| anyhow!(e))?;
        if let Some(exc) = r.get("exceptionDetails") {
            let txt = exc
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(|v| v.as_str())
                .or_else(|| exc.get("text").and_then(|v| v.as_str()))
                .unwrap_or("script exception");
            return Err(anyhow!(txt.to_string()));
        }
        Ok(r.get("result")
            .and_then(|res| res.get("value"))
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait]
impl Controller for ChromeController {
    async fn navigate(&self, target: &str) -> Result<()> {
        let url = if target.contains("://") || target.starts_with("about:") {
            target.to_string()
        } else {
            format!("https://{target}")
        };
        let r = self
            .cdp
            .send("Page.navigate", json!({ "url": url }))
            .await
            .map_err(|e| anyhow!(e))?;
        if let Some(err) = r.get("errorText").and_then(|v| v.as_str()) {
            return Err(anyhow!("navigation failed: {err}"));
        }
        // Wait for the document to finish loading.
        for _ in 0..100 {
            if self.eval("document.readyState").await.ok().and_then(|v| {
                v.as_str().map(|s| s == "complete")
            }) == Some(true)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let js = r##"(() => {
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
})()"##;
        let v = self.eval(js).await?;
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
                "(() => {{ const el=document.querySelector('[data-abf-ref=\"{r}\"]'); if(!el) throw new Error('no element @{r}'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})()"
            ),
            Locator::Css(s) => format!(
                "(() => {{ const el=document.querySelector({sel}); if(!el) throw new Error('no element'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})()",
                sel = js_str(s)
            ),
            Locator::Label(s) | Locator::Text(s) => format!(
                "(() => {{ const t={needle}.toLowerCase(); const el=[...document.querySelectorAll('a,button,input,select,textarea,[role],[onclick],summary')].find(e => ((e.innerText||e.value||e.getAttribute('aria-label')||'').trim().toLowerCase().includes(t)) && e.getBoundingClientRect().width>0); if(!el) throw new Error('no element with text'); el.scrollIntoView({{block:'center'}}); if(el.focus) try{{el.focus();}}catch(e){{}} el.click(); return true; }})()",
                needle = js_str(&s.to_lowercase())
            ),
            Locator::Role { role, name } => format!(
                "(() => {{ const role={r}; const nm={n}; const el=[...document.querySelectorAll('[role],a,button,input,select,textarea')].find(e => ((e.getAttribute('role')||e.tagName.toLowerCase())===role) && (!nm || ((e.innerText||e.value||e.getAttribute('aria-label')||'').toLowerCase().includes(nm)))); if(!el) throw new Error('no matching role'); el.scrollIntoView({{block:'center'}}); el.click(); return true; }})()",
                r = js_str(role),
                n = js_str(&name.clone().unwrap_or_default().to_lowercase())
            ),
            Locator::Point { x, y } => format!(
                "(() => {{ const el=document.elementFromPoint({x},{y}); if(!el) throw new Error('no element at point'); el.click(); return true; }})()"
            ),
        };
        self.eval(&js).await.map(|_| ())
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        self.cdp
            .send("Input.insertText", json!({ "text": text }))
            .await
            .map_err(|e| anyhow!(e))
            .map(|_| ())
    }

    async fn press(&self, key: &str) -> Result<()> {
        let (modifiers, base) = parse_key(key);
        let (k, code, vk) = key_info(&base)?;
        let base_evt = json!({
            "modifiers": modifiers, "key": k, "code": code, "windowsVirtualKeyCode": vk,
        });
        let mut down = base_evt.clone();
        down["type"] = json!("keyDown");
        let mut up = base_evt;
        up["type"] = json!("keyUp");
        self.cdp
            .send("Input.dispatchKeyEvent", down)
            .await
            .map_err(|e| anyhow!(e))?;
        self.cdp
            .send("Input.dispatchKeyEvent", up)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let a = amount.max(1);
        let (dx, dy) = match dir {
            ScrollDir::Up => (0, -a),
            ScrollDir::Down => (0, a),
            ScrollDir::Left => (-a, 0),
            ScrollDir::Right => (a, 0),
        };
        self.eval(&format!("(() => {{ window.scrollBy({dx}, {dy}); return true; }})()"))
            .await
            .map(|_| ())
    }

    async fn screenshot(&self) -> Result<Image> {
        let r = self
            .cdp
            .send("Page.captureScreenshot", json!({ "format": "png" }))
            .await
            .map_err(|e| anyhow!(e))?;
        let b64 = r
            .get("data")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("screenshot returned no data"))?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| anyhow!("decoding screenshot: {e}"))?;
        Ok(Image {
            data,
            format: "png".into(),
        })
    }

    fn backend(&self) -> Backend {
        Backend::Chrome
    }

    fn target(&self) -> String {
        self.target.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            eval: true,
            network: true,
            menus: false,
            coordinates: true,
            screenshot: true,
        }
    }
}

/// Parse `Cmd+Shift+A` into a CDP modifier bitmask + base key string.
/// CDP modifiers: Alt=1, Ctrl=2, Meta=4, Shift=8.
fn parse_key(spec: &str) -> (u32, String) {
    let mut modifiers = 0u32;
    let mut base = String::new();
    for part in spec.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "alt" | "option" | "opt" => modifiers |= 1,
            "ctrl" | "control" => modifiers |= 2,
            "cmd" | "command" | "meta" | "super" => modifiers |= 4,
            "shift" => modifiers |= 8,
            other => base = other.to_string(),
        }
    }
    (modifiers, base)
}

/// `(key, code, windowsVirtualKeyCode)` for a base key.
fn key_info(base: &str) -> Result<(String, String, i64)> {
    let info = match base {
        "enter" | "return" => ("Enter", "Enter", 13),
        "tab" => ("Tab", "Tab", 9),
        "escape" | "esc" => ("Escape", "Escape", 27),
        "backspace" => ("Backspace", "Backspace", 8),
        "delete" => ("Delete", "Delete", 46),
        "space" => (" ", "Space", 32),
        "up" => ("ArrowUp", "ArrowUp", 38),
        "down" => ("ArrowDown", "ArrowDown", 40),
        "left" => ("ArrowLeft", "ArrowLeft", 37),
        "right" => ("ArrowRight", "ArrowRight", 39),
        s if s.chars().count() == 1 => {
            let c = s.chars().next().unwrap();
            let up = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() {
                format!("Key{up}")
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            // Leak-free owned strings via a small allocation path below.
            return Ok((c.to_string(), code, up as i64));
        }
        other => return Err(anyhow!("unknown key: {other}")),
    };
    Ok((info.0.to_string(), info.1.to_string(), info.2))
}

/// Session-aware entry point for the Chrome backend.
pub struct ChromeFactory;

#[async_trait]
impl BackendFactory for ChromeFactory {
    fn backend(&self) -> Backend {
        Backend::Chrome
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let name = opts.session.clone().unwrap_or_else(|| "default".to_string());
        Ok(Identity {
            id: format!("chrome/{name}"),
            target: name,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        _opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        let name = rec.target.clone();
        let port = port_for(&name);
        // Reuse a running Chrome on this port, else launch one.
        let running = reqwest::get(format!("http://127.0.0.1:{port}/json/version"))
            .await
            .is_ok();
        let pid = if running {
            None
        } else {
            let profile_dir = store.paths(&rec.id)?.dir.join("profile");
            launch::launch(port, profile_dir).await?
        };
        let ws = launch::page_ws(port).await?;
        let cdp = CdpClient::connect(&ws).await?;
        // Page domain enables navigation lifecycle/screenshots.
        let _ = cdp.send("Page.enable", json!({})).await;
        rec.runtime.endpoint = Some(ws);
        rec.runtime.alive = true;
        if let Some(p) = pid {
            rec.runtime.pids = vec![p];
        }
        rec.config = json!({ "port": port });
        Ok(Box::new(ChromeController { cdp, target: name }))
    }
}
