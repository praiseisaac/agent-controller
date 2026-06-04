#![cfg(target_os = "macos")]
//! macOS desktop backend for `agent-controller`. macOS-only; empty elsewhere.
//!
//! AX-tree-first: `snapshot` reads the Accessibility tree (`@ref`s), semantic
//! locators act via `AXPress` (cursor-free), and refs/points fall back to
//! synthetic clicks delivered to the app via `CGEventPostToPid` (non-interruptive
//! by default; `--takeover` uses the global cursor). Screenshots via
//! `screencapture`.

pub mod ax;
pub mod display;
pub mod setup;
mod appres;
mod input;
mod snapshot_cache;
mod window;

pub use display::{displays, DisplayInfo};

use accessibility::AXUIElement;
use accessibility_sys::AXIsProcessTrusted;
use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Identity, Image, Locator, Options,
    Result, ScrollDir, SessionPaths, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use std::process::Command;

/// Error if Accessibility permission isn't granted to the controlling process.
fn require_ax_trust() -> Result<()> {
    if unsafe { AXIsProcessTrusted() } {
        Ok(())
    } else {
        Err(anyhow!(
            "Accessibility permission not granted. Enable it for your terminal app in \
             System Settings ▸ Privacy & Security ▸ Accessibility, then retry."
        ))
    }
}

pub struct MacController {
    pid: i32,
    target: String,
    paths: SessionPaths,
    takeover: bool,
}

impl MacController {
    fn root(&self) -> AXUIElement {
        ax::root_for(self.pid)
    }
}

#[async_trait]
impl Controller for MacController {
    async fn navigate(&self, target: &str) -> Result<()> {
        appres::launch(target)
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        require_ax_trust()?;
        let snap = ax::snapshot(&self.root());
        snapshot_cache::save(&self.paths.refs, &snap)?;
        Ok(snap)
    }

    async fn click(&self, loc: &Locator) -> Result<()> {
        require_ax_trust()?;
        match loc {
            Locator::Css(_) => Err(anyhow!("CSS selectors are not supported by the mac backend")),
            Locator::Point { x, y } => input::click(self.pid, *x, *y, self.takeover),
            Locator::Ref(r) => {
                let snap = snapshot_cache::load(&self.paths.refs)?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| &e.r#ref == r)
                    .ok_or_else(|| anyhow!("no cached element @{r}; run `snapshot` first"))?;
                let (cx, cy) = el.frame.center();
                input::click(self.pid, cx, cy, self.takeover)
            }
            Locator::Label(s) | Locator::Text(s) => {
                let needle = s.to_lowercase();
                let pred = move |_role: Option<&str>, label: Option<&str>| {
                    label.map(|l| l.to_lowercase().contains(&needle)).unwrap_or(false)
                };
                self.click_found(&pred, s)
            }
            Locator::Role { role, name } => {
                let want_role = role.clone();
                let want_name = name.clone().map(|n| n.to_lowercase());
                let pred = move |r: Option<&str>, label: Option<&str>| {
                    r == Some(want_role.as_str())
                        && match &want_name {
                            None => true,
                            Some(n) => label.map(|l| l.to_lowercase().contains(n)).unwrap_or(false),
                        }
                };
                self.click_found(&pred, role)
            }
        }
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        require_ax_trust()?;
        input::type_text(text)
    }

    async fn press(&self, key: &str) -> Result<()> {
        require_ax_trust()?;
        input::press(key)
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        input::scroll(self.pid, dir, amount, self.takeover)
    }

    async fn screenshot(&self) -> Result<Image> {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("agent-controller-shot-{}.png", std::process::id()));
        let mut cmd = Command::new("/usr/sbin/screencapture");
        cmd.arg("-x");
        // Prefer capturing the specific window by id (works even when occluded);
        // fall back to its on-screen region, then full screen.
        if let Some(wid) = window::frontmost_window_id(self.pid) {
            cmd.args(["-o", "-l", &wid.to_string()]);
        } else if let Some(f) = ax::first_window_frame(&self.root()) {
            cmd.arg(format!("-R{},{},{},{}", f.x, f.y, f.width, f.height));
        }
        cmd.arg(&tmp);
        let status = cmd.status().map_err(|e| anyhow!("screencapture: {e}"))?;
        if !status.success() {
            return Err(anyhow!(
                "screencapture failed (Screen Recording permission may be required)"
            ));
        }
        let data = std::fs::read(&tmp)?;
        let _ = std::fs::remove_file(&tmp);
        Ok(Image {
            data,
            format: "png".into(),
        })
    }

    async fn menu(&self, path: &str) -> Result<()> {
        require_ax_trust()?;
        let parts: Vec<String> = path.split('>').map(|s| s.trim().to_string()).collect();
        ax::press_menu_path(&self.root(), &parts)
    }

    fn backend(&self) -> Backend {
        Backend::Mac
    }

    fn target(&self) -> String {
        self.target.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            eval: false,
            network: false,
            menus: true,
            coordinates: true,
            screenshot: true,
        }
    }
}

impl MacController {
    /// Find an element via `pred`, prefer `AXPress`, fall back to a click at its
    /// frame center.
    fn click_found(
        &self,
        pred: &dyn Fn(Option<&str>, Option<&str>) -> bool,
        what: &str,
    ) -> Result<()> {
        let (el, frame) = ax::find(&self.root(), pred)
            .ok_or_else(|| anyhow!("no element matching {what:?}"))?;
        if ax::actions(&el).iter().any(|a| a == "AXPress") && ax::press(&el) {
            return Ok(());
        }
        if frame.is_empty() {
            return Err(anyhow!("matched {what:?} but it has no AXPress and no frame"));
        }
        let (cx, cy) = frame.center();
        input::click(self.pid, cx, cy, self.takeover)
    }
}

/// Session-aware entry point for the macOS backend.
pub struct MacFactory;

#[async_trait]
impl BackendFactory for MacFactory {
    fn backend(&self) -> Backend {
        Backend::Mac
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        require_ax_trust()?;
        let target = match &opts.app {
            Some(app) => app.clone(),
            None => appres::frontmost()?.1,
        };
        if target.is_empty() {
            return Err(anyhow!("could not determine a target app; pass --app <bundle|name>"));
        }
        let key = target.replace(['/', ' '], "-");
        Ok(Identity {
            id: format!("mac/{key}"),
            target,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        require_ax_trust()?;
        let pid = appres::launch_and_resolve(&rec.target)?;
        // Foreground-only model: bring the target app to the front and wait until
        // it is actually frontmost, so keystrokes/shortcuts/formatting land in its
        // key window (activation via `open` is asynchronous).
        appres::activate_and_wait(&rec.target, pid);
        rec.runtime.pids = vec![pid as u32];
        rec.runtime.alive = true;
        rec.config = serde_json::json!({ "pid": pid });
        let paths = store.paths(&rec.id)?;
        Ok(Box::new(MacController {
            pid,
            target: rec.target.clone(),
            paths,
            takeover: opts.takeover,
        }))
    }
}
