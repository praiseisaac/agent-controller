//! Firefox backend for `agent-controller`, driving Firefox over WebDriver BiDi.
//!
//! A BiDi session is bound to its WebSocket connection, so control needs a
//! long-lived process to own that connection — the **firefox daemon** (see
//! `daemon.rs`). The CLI is a thin client: `FirefoxController` sends action
//! requests over a localhost TCP socket; the daemon runs them against the live
//! [`BidiSession`]. Instances are keyed by `--session <name>` (default
//! `default`) → an isolated profile + debug port + daemon.
//!
//! ## Automation guidance
//! - Upload files with the `upload @ref <abs path>` verb (BiDi `input.setFiles`):
//!   in-band, no OS picker, no focus stealing, works even on a background tab —
//!   do not open the native dialog.
//! - Do not `pkill __firefox-daemon` while in use: it orphans the BiDi session
//!   and wedges that Firefox (restart the browser to recover).
//! - `@ref`s are `data-abf-ref` DOM attributes; re-snapshot after navigation or a
//!   re-render. Canonical guidance: `app/src/guidance.rs`.

pub mod bidi;
pub mod daemon;
pub mod ipc;
mod launch;

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image, Locator,
    Options, Rect, Result, ScrollDir, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use base64::Engine;
use ipc::{daemon_addr, Req};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Deterministic debug port (9300..=9399) from the instance name.
fn port_for(name: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    9300 + (h % 100) as u16
}

/// Ensure a Firefox is serving BiDi for `name`; returns the ws base url and, if
/// launched this call, its pid. Used by the daemon (which then owns the ws).
pub(crate) async fn ensure(name: &str, profile_dir: PathBuf) -> Result<(String, Option<u32>)> {
    let port = port_for(name);
    let ws = format!("ws://127.0.0.1:{port}");
    if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
        return Ok((ws, None));
    }
    let launched = launch::launch(port, false, profile_dir)
        .await
        .map_err(|e| anyhow!("launching Firefox: {e}"))?;
    let pid = launched.child.id();
    drop(launched); // kill_on_drop is false → Firefox persists
    Ok((ws, pid))
}

/// Daemon entry point, invoked by the binary's hidden `__firefox-daemon` command.
pub async fn run_daemon(name: String, profile_dir: PathBuf, addr: String) -> Result<()> {
    daemon::run(name, profile_dir, addr).await
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

pub struct FirefoxController {
    addr: String,
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

impl FirefoxController {
    async fn call(&self, op: &str, args: Vec<String>, n: i64) -> Result<serde_json::Value> {
        let resp = ipc::request(&self.addr, &Req { op: op.into(), args, n }).await?;
        if resp.ok {
            Ok(resp.data.unwrap_or(serde_json::Value::Null))
        } else {
            Err(anyhow!(resp.error.unwrap_or_else(|| "daemon error".into())))
        }
    }

    async fn eval(&self, js: &str) -> Result<serde_json::Value> {
        self.call("eval", vec![js.to_string()], 0).await
    }
}

#[async_trait]
impl Controller for FirefoxController {
    async fn navigate(&self, target: &str) -> Result<()> {
        let url = if target.contains("://") || target.starts_with("about:") {
            target.to_string()
        } else {
            format!("https://{target}")
        };
        self.call("navigate", vec![url], 0).await.map(|_| ())
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
  return JSON.stringify(out);
})()"##;
        let v = self.eval(js).await?;
        let json = v.as_str().unwrap_or("[]");
        let raws: Vec<RawEl> = serde_json::from_str(json).unwrap_or_default();
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
        self.call("type", vec![text.to_string()], 0).await.map(|_| ())
    }

    async fn press(&self, key: &str) -> Result<()> {
        self.call("press", vec![key.to_string()], 0).await.map(|_| ())
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let d = match dir {
            ScrollDir::Up => "up",
            ScrollDir::Down => "down",
            ScrollDir::Left => "left",
            ScrollDir::Right => "right",
        };
        self.call("scroll", vec![d.to_string()], amount as i64)
            .await
            .map(|_| ())
    }

    async fn screenshot(&self) -> Result<Image> {
        let v = self.call("screenshot", vec![], 0).await?;
        let b64 = v.as_str().ok_or_else(|| anyhow!("screenshot returned no data"))?;
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
        let mut args = vec![selector];
        args.extend(paths.iter().cloned());
        self.call("setfiles", args, 0).await.map(|_| ())
    }

    fn backend(&self) -> Backend {
        Backend::Firefox
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

/// Ensure the firefox daemon for `name` is running and reachable.
async fn ensure_daemon(name: &str, profile_dir: &Path, addr: &str) -> Result<()> {
    let ping = Req {
        op: "ping".into(),
        args: vec![],
        n: 0,
    };
    if ipc::request(addr, &ping).await.is_ok() {
        return Ok(());
    }
    // Spawn ourselves in daemon mode, detached.
    let exe = std::env::current_exe()?;
    let mut log = std::env::temp_dir();
    log.push("agent-controller");
    let _ = std::fs::create_dir_all(&log);
    log.push(format!("firefox-daemon-{name}.log"));
    let out = std::fs::File::create(&log)?;
    let err = out.try_clone()?;
    std::process::Command::new(exe)
        .args([
            "__firefox-daemon",
            "--session",
            name,
            "--profile",
            &profile_dir.to_string_lossy(),
            "--addr",
            addr,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| anyhow!("spawning firefox daemon: {e}"))?;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if ipc::request(addr, &ping).await.is_ok() {
            return Ok(());
        }
    }
    Err(anyhow!(
        "firefox daemon did not become ready; see {}",
        log.display()
    ))
}

/// Session-aware entry point for the Firefox backend.
pub struct FirefoxFactory;

#[async_trait]
impl BackendFactory for FirefoxFactory {
    fn backend(&self) -> Backend {
        Backend::Firefox
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let name = opts.session.clone().unwrap_or_else(|| "default".to_string());
        Ok(Identity {
            id: format!("firefox/{name}"),
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
        let profile_dir = store.paths(&rec.id)?.dir.join("profile");
        let addr = daemon_addr(&name);
        ensure_daemon(&name, &profile_dir, &addr).await?;
        rec.runtime.endpoint = Some(addr.clone());
        rec.runtime.alive = true;
        rec.config = serde_json::json!({ "port": port_for(&name), "daemon": addr });
        Ok(Box::new(FirefoxController {
            addr,
            target: name,
        }))
    }
}
