//! Chrome backend for `agent-controller`, driving Chrome over the DevTools
//! Protocol (CDP). Unlike Firefox's BiDi, CDP permits transient connections, so
//! each CLI invocation reconnects to a persistent Chrome — no daemon needed.
//! Instances are keyed by `--session <name>` (default `default`) → an isolated
//! profile + debug port.
//!
//! ## Automation guidance
//! - Upload files with `upload @ref <abs path>` (CDP `DOM.setFileInputFiles`):
//!   in-band, no OS picker, no focus stealing.
//! - Re-snapshot after the page changes; stale `@ref`s fail with "no element".
//! - Windows cold-start at 1360x800 (~1.7:1) unless configured
//!   (`Options::launch`); a running Chrome is not resized.
//! - `AGENT_CONTROLLER_REALISTIC=1` drives the page like a person: curved,
//!   eased pointer moves, a beat before each click, uneven typing, and a drawn
//!   cursor a screencast shows (`realistic.rs`). Off, input is instant.
//!   Canonical guidance: `app/src/guidance.rs`.

mod cdp;
mod launch;
pub mod realistic;

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image,
    LaunchConfig, Locator, Options, Rect, Result, ScrollDir, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use base64::Engine;
use cdp::CdpClient;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    /// Realistic input (`AGENT_CONTROLLER_REALISTIC`); `None` is instant input.
    realistic: Option<Realism>,
}

/// A realistic session's state: its timing source, and where the pointer was
/// left (the session folder's `pointer.json`) so the next call — a separate
/// process — glides on from there instead of jumping.
struct Realism {
    rng: Mutex<realistic::Rng>,
    pointer_file: PathBuf,
}

#[derive(Deserialize)]
struct Box2 {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
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

    async fn send(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        self.cdp.send(method, params).await.map_err(|e| anyhow!(e))
    }

    /// Draw the cursor into this document and every one this connection loads,
    /// at the pointer's last place. Best effort: a page mid-load may refuse it.
    async fn ensure_cursor(&self) {
        let Some(r) = &self.realistic else { return };
        let _ = self
            .send(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": realistic::CURSOR_JS }),
            )
            .await;
        let _ = self.eval(realistic::CURSOR_JS).await;
        if let Some((x, y)) = read_pointer(&r.pointer_file) {
            let _ = self
                .eval(&format!(
                    "window.__acCursorAt && window.__acCursorAt({x},{y})"
                ))
                .await;
        }
    }

    fn rng<T>(&self, f: impl FnOnce(&mut realistic::Rng) -> T) -> Option<T> {
        let r = self.realistic.as_ref()?;
        let mut rng = r.rng.lock().unwrap_or_else(|e| e.into_inner());
        Some(f(&mut rng))
    }

    /// Glide the pointer from where it was left to `(x, y)` with real mouse
    /// events, the drawn cursor following.
    async fn glide(&self, x: f64, y: f64) -> Result<()> {
        let Some(r) = &self.realistic else {
            return Ok(());
        };
        let from = match read_pointer(&r.pointer_file) {
            Some(p) => p,
            None => {
                let v = self.eval("[innerWidth / 2, innerHeight / 2]").await?;
                (
                    v[0].as_f64().unwrap_or(680.0),
                    v[1].as_f64().unwrap_or(400.0),
                )
            }
        };
        let steps = self
            .rng(|rng| realistic::mouse_path(rng, from, (x, y)))
            .unwrap_or_default();
        let _ = self.eval("window.__acDriving = true").await;
        for s in steps {
            self.send(
                "Input.dispatchMouseEvent",
                json!({ "type": "mouseMoved", "x": s.x, "y": s.y }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(s.wait_ms)).await;
        }
        let _ = std::fs::write(&r.pointer_file, json!({ "x": x, "y": y }).to_string());
        Ok(())
    }

    /// A person's click: find the element, glide to a point near its centre,
    /// wait a beat, press and release there.
    async fn realistic_click(&self, loc: &Locator) -> Result<()> {
        let (x, y) = match loc {
            Locator::Point { x, y } => (*x, *y),
            _ => {
                let v = self.eval(&locate_box_js(loc)).await?;
                let b: Box2 = serde_json::from_value(v).map_err(|e| anyhow!("element box: {e}"))?;
                self.rng(|rng| realistic::click_point(rng, b.x, b.y, b.w, b.h))
                    .unwrap_or((b.x + b.w / 2.0, b.y + b.h / 2.0))
            }
        };
        self.ensure_cursor().await;
        self.glide(x, y).await?;
        let pause = self.rng(realistic::click_pause_ms).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(pause)).await;
        let _ = self.eval("window.__acDriving = true").await;
        for kind in ["mousePressed", "mouseReleased"] {
            self.send(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            )
            .await?;
            let hold = self.rng(realistic::hold_ms).unwrap_or(0);
            tokio::time::sleep(Duration::from_millis(hold)).await;
        }
        let _ = self.eval("window.__acDriving = false").await;
        // the click may have loaded a new document: draw the cursor into it too
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.ensure_cursor().await;
        Ok(())
    }

    /// A person's typing: one key at a time, in the uneven rhythm of
    /// `realistic::typing`. A character with no key of its own (accents,
    /// emoji, newline) is inserted as text, as instant typing does.
    async fn realistic_type(&self, text: &str) -> Result<()> {
        let strokes = self
            .rng(|rng| realistic::typing(rng, text))
            .unwrap_or_default();
        for k in strokes {
            if k.ch.is_ascii_graphic() || k.ch == ' ' {
                let s = k.ch.to_string();
                let mut down =
                    json!({ "type": "keyDown", "key": s, "text": s, "unmodifiedText": s });
                let mut up = json!({ "type": "keyUp", "key": s });
                // a letter, digit or space carries its code; a symbol's would
                // name another key (`.` is 46, Delete), so it goes by text alone
                if k.ch.is_ascii_alphanumeric() || k.ch == ' ' {
                    let (_, code, vk) = key_info(&if k.ch == ' ' {
                        "space".into()
                    } else {
                        k.ch.to_ascii_lowercase().to_string()
                    })?;
                    for e in [&mut down, &mut up] {
                        e["code"] = json!(code);
                        e["windowsVirtualKeyCode"] = json!(vk);
                    }
                }
                self.send("Input.dispatchKeyEvent", down).await?;
                tokio::time::sleep(Duration::from_millis(k.hold_ms)).await;
                self.send("Input.dispatchKeyEvent", up).await?;
            } else {
                self.send("Input.insertText", json!({ "text": k.ch.to_string() }))
                    .await?;
            }
            tokio::time::sleep(Duration::from_millis(k.gap_ms)).await;
        }
        Ok(())
    }
}

/// The pointer's last place in a realistic session, if one was saved.
fn read_pointer(file: &PathBuf) -> Option<(f64, f64)> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}

/// JS that finds the element a locator names, scrolls it into view, and
/// returns its box — the realistic click's target. Finds as `click` does.
fn locate_box_js(loc: &Locator) -> String {
    let find = match loc {
        Locator::Ref(r) => format!(
            "document.querySelector('[data-abf-ref=\"{r}\"]') || (() => {{ throw new Error('no element @{r}'); }})()"
        ),
        Locator::Css(s) => format!(
            "document.querySelector({sel}) || (() => {{ throw new Error('no element'); }})()",
            sel = js_str(s)
        ),
        Locator::Label(s) | Locator::Text(s) => format!(
            "(() => {{ const t={needle}.toLowerCase(); return [...document.querySelectorAll('a,button,input,select,textarea,[role],[onclick],summary')].find(e => ((e.innerText||e.value||e.getAttribute('aria-label')||'').trim().toLowerCase().includes(t)) && e.getBoundingClientRect().width>0); }})() || (() => {{ throw new Error('no element with text'); }})()",
            needle = js_str(&s.to_lowercase())
        ),
        Locator::Role { role, name } => format!(
            "(() => {{ const role={r}; const nm={n}; return [...document.querySelectorAll('[role],a,button,input,select,textarea')].find(e => ((e.getAttribute('role')||e.tagName.toLowerCase())===role) && (!nm || ((e.innerText||e.value||e.getAttribute('aria-label')||'').toLowerCase().includes(nm)))); }})() || (() => {{ throw new Error('no matching role'); }})()",
            r = js_str(role),
            n = js_str(&name.clone().unwrap_or_default().to_lowercase())
        ),
        Locator::Point { x, y } => format!("document.elementFromPoint({x},{y})"),
    };
    format!(
        "(async () => {{ const el = {find}; el.scrollIntoView({{block:'center'}}); await new Promise(r => requestAnimationFrame(() => r())); const b = el.getBoundingClientRect(); return {{ x: b.left, y: b.top, w: b.width, h: b.height }}; }})()"
    )
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
        self.ensure_cursor().await;
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
        if self.realistic.is_some() {
            return self.realistic_click(loc).await;
        }
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
        if self.realistic.is_some() {
            return self.realistic_type(text).await;
        }
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
        if let Some(hold) = self.rng(realistic::hold_ms) {
            tokio::time::sleep(Duration::from_millis(hold)).await;
        }
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
        self.ensure_cursor().await;
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

    async fn set_files(&self, loc: &Locator, paths: &[String]) -> Result<()> {
        let selector = match loc {
            Locator::Ref(r) => format!("[data-abf-ref=\"{r}\"]"),
            Locator::Css(s) => s.clone(),
            _ => {
                return Err(anyhow!(
                    "upload requires an @ref or css: locator addressing the file <input>"
                ))
            }
        };
        // Resolve the input to a RemoteObject (objectId), then attach the files
        // in-band via DOM.setFileInputFiles — no native picker.
        let r = self
            .cdp
            .send(
                "Runtime.evaluate",
                json!({ "expression": format!("document.querySelector({})", js_str(&selector)), "returnByValue": false }),
            )
            .await
            .map_err(|e| anyhow!(e))?;
        let object_id = r
            .get("result")
            .and_then(|res| res.get("objectId"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("no element matches {selector}"))?;
        self.cdp
            .send("DOM.setFileInputFiles", json!({ "files": paths, "objectId": object_id }))
            .await
            .map_err(|e| anyhow!(e))
            .map(|_| ())
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
            upload: true,
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
        opts: &Options,
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
            launch::launch(port, profile_dir, &opts.launch).await?
        };
        let ws = launch::page_ws(port).await?;
        let cdp = CdpClient::connect(&ws).await?;
        // Page domain enables navigation lifecycle/screenshots.
        let _ = cdp.send("Page.enable", json!({})).await;
        if !running {
            // `--window-size` sets the initial bounds, but a profile's saved
            // window state can win on some platforms; pin them over CDP too.
            set_window_bounds(&cdp, &opts.launch).await?;
        }
        rec.runtime.endpoint = Some(ws);
        rec.runtime.alive = true;
        if let Some(p) = pid {
            rec.runtime.pids = vec![p];
        }
        // Record the launch settings the running browser was started with; a
        // resumed session keeps the ones it launched under.
        let launched_with = if running {
            rec.config.get("launch").cloned().unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::to_value(&opts.launch).unwrap_or(serde_json::Value::Null)
        };
        rec.config = json!({ "port": port, "launch": launched_with });
        let realistic =
            if realistic::enabled(std::env::var(realistic::ENV_REALISTIC).ok().as_deref()) {
                let seed =
                    realistic::seed(std::env::var(realistic::ENV_REALISTIC_SEED).ok().as_deref());
                Some(Realism {
                    rng: Mutex::new(realistic::Rng::new(seed)),
                    pointer_file: store.paths(&rec.id)?.dir.join("pointer.json"),
                })
            } else {
                None
            };
        let ctl = ChromeController {
            cdp,
            target: name,
            realistic,
        };
        ctl.ensure_cursor().await;
        Ok(Box::new(ctl))
    }
}

/// Apply the configured window geometry to the page's window over CDP
/// (`Browser.setWindowBounds`). Headless Chrome has no window; skip it there.
async fn set_window_bounds(cdp: &CdpClient, launch: &LaunchConfig) -> Result<()> {
    if launch.is_headless() {
        return Ok(());
    }
    let win = cdp
        .send("Browser.getWindowForTarget", json!({}))
        .await
        .map_err(|e| anyhow!("looking up Chrome window: {e}"))?;
    let Some(window_id) = win.get("windowId").cloned() else {
        return Ok(());
    };
    let (w, h) = launch.window_size();
    let mut bounds = json!({ "windowState": "normal", "width": w, "height": h });
    if let Some((x, y)) = launch.window_position() {
        bounds["left"] = json!(x);
        bounds["top"] = json!(y);
    }
    cdp.send(
        "Browser.setWindowBounds",
        json!({ "windowId": window_id, "bounds": bounds }),
    )
    .await
    .map_err(|e| anyhow!("sizing Chrome window: {e}"))?;
    Ok(())
}
