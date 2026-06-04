#![cfg(target_os = "macos")]
//! iOS Simulator backend for `agent-controller`, driven over gRPC against
//! `idb_companion`. macOS-only; compiles to an empty crate elsewhere.
//!
//! Implements the core [`Controller`] trait:
//! `describe` → `@ref` snapshot, `hid` → taps/typing/swipes, `screenshot` →
//! PNG. App launch / URL open / boot go through `xcrun simctl` (reliable and
//! dependency-free); UI interaction goes through idb's gRPC.

pub mod idb {
    tonic::include_proto!("idb");
}

mod companion;
mod keymap;
mod snapshot;

use agent_controller_core::{
    anyhow, BackendFactory, Capabilities, Controller, Identity, Image, Locator, Options, Result,
    ScrollDir, SessionPaths, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use std::process::Command;
use tonic::transport::Channel;

use idb::companion_service_client::CompanionServiceClient;
use idb::hid_event::{
    self, hid_press_action, HidDirection, HidKey, HidPress, HidPressAction, HidSwipe, HidTouch,
};

/// A controller bound to one booted simulator.
pub struct IosSimController {
    udid: String,
    endpoint: String,
    paths: SessionPaths,
}

impl IosSimController {
    /// Construct from an already-resolved udid, companion endpoint, and the
    /// session's artifact/cache paths.
    pub fn open(udid: String, endpoint: String, paths: SessionPaths) -> Self {
        Self {
            udid,
            endpoint,
            paths,
        }
    }

    /// Convenience standalone constructor (no session store): binds to `udid`
    /// (or the single booted sim), booting + starting a companion, caching into
    /// the temp dir.
    pub async fn connect(udid: Option<String>) -> Result<Self> {
        let udid = match udid {
            Some(u) => u,
            None => booted_udid()?,
        };
        ensure_booted(&udid)?;
        let mut log = std::env::temp_dir();
        log.push("agent-controller");
        let _ = std::fs::create_dir_all(&log);
        log.push(format!("companion-{udid}.log"));
        let (endpoint, _pid) = companion::ensure(&udid, &log).await?;
        let mut base = std::env::temp_dir();
        base.push("agent-controller");
        let paths = SessionPaths {
            refs: base.join(format!("ios-{udid}-refs.json")),
            screenshots: base.clone(),
            files: base.clone(),
            log,
            dir: base,
        };
        Ok(Self::open(udid, endpoint, paths))
    }

    pub fn udid(&self) -> &str {
        &self.udid
    }

    async fn client(&self) -> Result<CompanionServiceClient<Channel>> {
        CompanionServiceClient::connect(self.endpoint.clone())
            .await
            .map_err(|e| anyhow!("connecting to companion at {}: {e}", self.endpoint))
    }

    /// Fresh accessibility tree (not cached).
    async fn live_snapshot(&self) -> Result<Snapshot> {
        let mut c = self.client().await?;
        let req = idb::AccessibilityInfoRequest {
            point: None,
            format: idb::accessibility_info_request::Format::Legacy as i32,
        };
        let resp = c
            .accessibility_info(req)
            .await
            .map_err(|s| anyhow!("accessibility_info: {}", s.message()))?
            .into_inner();
        snapshot::parse(&resp.json)
    }

    /// Resolve a locator to an on-screen point (logical points).
    async fn resolve(&self, loc: &Locator) -> Result<(f64, f64)> {
        match loc {
            Locator::Css(_) => Err(anyhow!("CSS selectors are not supported by ios-sim")),
            Locator::Point { x, y } => Ok((*x, *y)),
            Locator::Ref(r) => {
                let snap = snapshot::load_cache(&self.paths.refs)?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| &e.r#ref == r)
                    .ok_or_else(|| anyhow!("no cached element @{r}; run `snapshot` first"))?;
                Ok(el.frame.center())
            }
            Locator::Label(s) | Locator::Text(s) => {
                let needle = s.to_lowercase();
                let snap = self.live_snapshot().await?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| {
                        e.label
                            .as_deref()
                            .map(|l| l.to_lowercase().contains(&needle))
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| anyhow!("no element with label containing {s:?}"))?;
                Ok(el.frame.center())
            }
            Locator::Role { role, name } => {
                let snap = self.live_snapshot().await?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| {
                        e.role.as_deref() == Some(role.as_str())
                            && match name {
                                None => true,
                                Some(n) => e
                                    .label
                                    .as_deref()
                                    .map(|l| l.to_lowercase().contains(&n.to_lowercase()))
                                    .unwrap_or(false),
                            }
                    })
                    .ok_or_else(|| anyhow!("no {role} matching {name:?}"))?;
                Ok(el.frame.center())
            }
        }
    }

    async fn send_hid(&self, events: Vec<idb::HidEvent>) -> Result<()> {
        let mut c = self.client().await?;
        c.hid(tokio_stream::iter(events))
            .await
            .map_err(|s| anyhow!("hid: {}", s.message()))?;
        Ok(())
    }

    async fn tap_point(&self, x: f64, y: f64) -> Result<()> {
        self.send_hid(vec![
            touch_event(x, y, HidDirection::Down),
            touch_event(x, y, HidDirection::Up),
        ])
        .await
    }

    /// Logical screen size in points, from `describe`.
    async fn screen_size(&self) -> Result<(f64, f64)> {
        let mut c = self.client().await?;
        let resp = c
            .describe(idb::TargetDescriptionRequest {
                fetch_diagnostics: false,
            })
            .await
            .map_err(|s| anyhow!("describe: {}", s.message()))?
            .into_inner();
        let dims = resp
            .target_description
            .and_then(|t| t.screen_dimensions)
            .ok_or_else(|| anyhow!("no screen dimensions reported"))?;
        let w = if dims.width_points > 0 {
            dims.width_points
        } else {
            dims.width
        };
        let h = if dims.height_points > 0 {
            dims.height_points
        } else {
            dims.height
        };
        Ok((w as f64, h as f64))
    }
}

#[async_trait]
impl Controller for IosSimController {
    async fn navigate(&self, target: &str) -> Result<()> {
        if target.contains("://") {
            simctl(&["openurl", &self.udid, target])?;
        } else if target.eq_ignore_ascii_case("home") {
            // Home button via HID.
            let btn = |dir: HidDirection| idb::HidEvent {
                event: Some(hid_event::Event::Press(HidPress {
                    action: Some(HidPressAction {
                        action: Some(hid_press_action::Action::Button(hid_event::HidButton {
                            button: hid_event::HidButtonType::Home as i32,
                        })),
                    }),
                    direction: dir as i32,
                })),
            };
            self.send_hid(vec![btn(HidDirection::Down), btn(HidDirection::Up)])
                .await?;
        } else {
            // Treat as a bundle id. Cold-start (terminate then launch) so the app
            // opens to a predictable initial state rather than restoring whatever
            // screen it was last left on.
            let _ = simctl(&["terminate", &self.udid, target]);
            simctl(&["launch", &self.udid, target])?;
        }
        Ok(())
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let snap = self.live_snapshot().await?;
        snapshot::save_cache(&self.paths.refs, &snap)?;
        Ok(snap)
    }

    async fn click(&self, loc: &Locator) -> Result<()> {
        let (x, y) = self.resolve(loc).await?;
        self.tap_point(x, y).await
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        let mut events = Vec::new();
        for c in text.chars() {
            let Some((code, shift)) = keymap::char_to_hid(c) else {
                return Err(anyhow!("cannot type character {c:?}"));
            };
            if shift {
                events.push(key_event(keymap::SHIFT, HidDirection::Down));
            }
            events.push(key_event(code, HidDirection::Down));
            events.push(key_event(code, HidDirection::Up));
            if shift {
                events.push(key_event(keymap::SHIFT, HidDirection::Up));
            }
        }
        if events.is_empty() {
            return Ok(());
        }
        self.send_hid(events).await
    }

    async fn press(&self, key: &str) -> Result<()> {
        if let Some(code) = keymap::named_key(key) {
            return self
                .send_hid(vec![
                    key_event(code, HidDirection::Down),
                    key_event(code, HidDirection::Up),
                ])
                .await;
        }
        let button = match key.to_ascii_lowercase().as_str() {
            "home" => hid_event::HidButtonType::Home,
            "lock" => hid_event::HidButtonType::Lock,
            "siri" => hid_event::HidButtonType::Siri,
            "side" | "side-button" => hid_event::HidButtonType::SideButton,
            other => return Err(anyhow!("unknown key/button: {other}")),
        };
        let btn = |dir: HidDirection| idb::HidEvent {
            event: Some(hid_event::Event::Press(HidPress {
                action: Some(HidPressAction {
                    action: Some(hid_press_action::Action::Button(hid_event::HidButton {
                        button: button as i32,
                    })),
                }),
                direction: dir as i32,
            })),
        };
        self.send_hid(vec![btn(HidDirection::Down), btn(HidDirection::Up)])
            .await
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let (w, h) = self.screen_size().await?;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let d = amount.max(1) as f64 / 2.0;
        // To reveal content in `dir`, the finger swipes the opposite way.
        let (sx, sy, ex, ey) = match dir {
            ScrollDir::Down => (cx, cy + d, cx, cy - d),
            ScrollDir::Up => (cx, cy - d, cx, cy + d),
            ScrollDir::Left => (cx + d, cy, cx - d, cy),
            ScrollDir::Right => (cx - d, cy, cx + d, cy),
        };
        let swipe = idb::HidEvent {
            event: Some(hid_event::Event::Swipe(HidSwipe {
                start: Some(idb::Point { x: sx, y: sy }),
                end: Some(idb::Point { x: ex, y: ey }),
                delta: 0.0,
                duration: 0.3,
            })),
        };
        self.send_hid(vec![swipe]).await
    }

    async fn screenshot(&self) -> Result<Image> {
        let mut c = self.client().await?;
        let resp = c
            .screenshot(idb::ScreenshotRequest {})
            .await
            .map_err(|s| anyhow!("screenshot: {}", s.message()))?
            .into_inner();
        let format = if resp.image_format.is_empty() {
            "png".to_string()
        } else {
            resp.image_format
        };
        // idb returns the framebuffer at native Retina pixels (e.g. 1206x2622),
        // but the accessibility tree and taps both work in logical points (e.g.
        // 402x874). Downscale to points so a coordinate read off the screenshot
        // is the same coordinate a tap expects — one space everywhere.
        let data = match self.screen_size().await {
            Ok((w, h)) => fit_to_points(&resp.image_data, &format, w, h)
                .unwrap_or(resp.image_data),
            Err(_) => resp.image_data,
        };
        Ok(Image { data, format })
    }

    fn backend(&self) -> agent_controller_core::Backend {
        agent_controller_core::Backend::IosSim
    }

    fn target(&self) -> String {
        self.udid.clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            eval: false,
            network: false,
            menus: false,
            coordinates: true,
            screenshot: true,
        }
    }
}

/// Session-aware entry point for the iOS Simulator backend.
pub struct IosSimFactory;

#[async_trait]
impl BackendFactory for IosSimFactory {
    fn backend(&self) -> agent_controller_core::Backend {
        agent_controller_core::Backend::IosSim
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let udid = match &opts.udid {
            Some(u) => u.clone(),
            None => booted_udid()?,
        };
        Ok(Identity {
            id: format!("ios-sim/{udid}"),
            target: udid,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        _opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        let udid = rec.target.clone();
        ensure_booted(&udid)?;
        let paths = store.paths(&rec.id)?;
        let (endpoint, pid) = companion::ensure(&udid, &paths.log).await?;
        rec.runtime.endpoint = Some(endpoint.clone());
        rec.runtime.alive = true;
        if let Some(p) = pid {
            if !rec.runtime.pids.contains(&p) {
                rec.runtime.pids.push(p);
            }
        }
        rec.config = serde_json::json!({ "grpc_port": companion::port_for(&udid) });
        Ok(Box::new(IosSimController::open(udid, endpoint, paths)))
    }
}

/// Resize a PNG framebuffer to logical-point dimensions so screenshot pixels
/// line up 1:1 with the coordinate space used by the snapshot tree and taps.
/// Returns `None` (caller keeps the original) on a non-PNG format, a decode
/// failure, or when the image is already at (or below) point size.
fn fit_to_points(data: &[u8], format: &str, w_pts: f64, h_pts: f64) -> Option<Vec<u8>> {
    if !format.eq_ignore_ascii_case("png") || w_pts <= 0.0 || h_pts <= 0.0 {
        return None;
    }
    let img = image::load_from_memory(data).ok()?;
    let (tw, th) = (w_pts.round() as u32, h_pts.round() as u32);
    if tw == 0 || th == 0 || (img.width() <= tw && img.height() <= th) {
        return None; // already point-sized (1x device) — nothing to do
    }
    let scaled = img.resize_exact(tw, th, image::imageops::FilterType::Triangle);
    let mut out = std::io::Cursor::new(Vec::new());
    scaled
        .write_to(&mut out, image::ImageFormat::Png)
        .ok()?;
    Some(out.into_inner())
}

// --- HID event constructors ---

fn touch_event(x: f64, y: f64, dir: HidDirection) -> idb::HidEvent {
    idb::HidEvent {
        event: Some(hid_event::Event::Press(HidPress {
            action: Some(HidPressAction {
                action: Some(hid_press_action::Action::Touch(HidTouch {
                    point: Some(idb::Point { x, y }),
                })),
            }),
            direction: dir as i32,
        })),
    }
}

fn key_event(code: u64, dir: HidDirection) -> idb::HidEvent {
    idb::HidEvent {
        event: Some(hid_event::Event::Press(HidPress {
            action: Some(HidPressAction {
                action: Some(hid_press_action::Action::Key(HidKey { keycode: code })),
            }),
            direction: dir as i32,
        })),
    }
}

// --- simctl helpers ---

fn simctl(args: &[&str]) -> Result<String> {
    let out = Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .output()
        .map_err(|e| anyhow!("running `xcrun simctl {}`: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow!(
            "`xcrun simctl {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// UDID of the single booted simulator (errors if zero or many).
pub fn booted_udid() -> Result<String> {
    let json = simctl(&["list", "devices", "booted", "-j"])?;
    let v: serde_json::Value = serde_json::from_str(&json)?;
    let mut found = Vec::new();
    if let Some(devices) = v.get("devices").and_then(|d| d.as_object()) {
        for (_runtime, list) in devices {
            if let Some(arr) = list.as_array() {
                for dev in arr {
                    if dev.get("state").and_then(|s| s.as_str()) == Some("Booted") {
                        if let Some(udid) = dev.get("udid").and_then(|u| u.as_str()) {
                            found.push(udid.to_string());
                        }
                    }
                }
            }
        }
    }
    match found.len() {
        1 => Ok(found.pop().unwrap()),
        0 => Err(anyhow!(
            "no booted simulator; boot one (e.g. `xcrun simctl boot <udid>`) or pass --udid"
        )),
        _ => Err(anyhow!(
            "multiple booted simulators ({}); pass --udid to choose",
            found.join(", ")
        )),
    }
}

/// Boot `udid` if not already booted, waiting until ready. Quiet when the sim is
/// already booted (avoids `simctl bootstatus` chatter on every command).
pub fn ensure_booted(udid: &str) -> Result<()> {
    let json = simctl(&["list", "devices", "-j"])?;
    let v: serde_json::Value = serde_json::from_str(&json)?;
    let already = v
        .get("devices")
        .and_then(|d| d.as_object())
        .map(|runtimes| {
            runtimes.values().any(|list| {
                list.as_array().is_some_and(|arr| {
                    arr.iter().any(|dev| {
                        dev.get("udid").and_then(|u| u.as_str()) == Some(udid)
                            && dev.get("state").and_then(|s| s.as_str()) == Some("Booted")
                    })
                })
            })
        })
        .unwrap_or(false);
    if already {
        return Ok(());
    }
    // Boot and block until ready; swallow stdout chatter.
    let _ = Command::new("xcrun")
        .args(["simctl", "bootstatus", udid, "-b"])
        .stdout(std::process::Stdio::null())
        .status()
        .map_err(|e| anyhow!("running simctl bootstatus: {e}"))?;
    Ok(())
}
