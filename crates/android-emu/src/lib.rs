//! Android emulator (or device) backend, driven via `adb`. Host-OS-agnostic.
//! The Android analogue of `ios-sim`: `uiautomator dump` → `@ref` snapshot,
//! `input tap/text/keyevent/swipe` for actions, `screencap` for screenshots.
//! Instances are keyed by device serial (`--session <serial>`, default the
//! single running device).

mod adb;
mod snapshot;

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Identity, Image, Locator, Options,
    Result, ScrollDir, SessionPaths, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;

pub struct AndroidEmuController {
    serial: String,
    paths: SessionPaths,
}

impl AndroidEmuController {
    fn sh(&self, args: &[&str]) -> Result<String> {
        adb::adb(Some(&self.serial), args)
    }

    async fn live_snapshot(&self) -> Result<Snapshot> {
        // Dump the hierarchy to a file, then read it back (robust vs /dev/tty noise).
        self.sh(&["shell", "uiautomator", "dump", "/sdcard/window_dump.xml"])?;
        let xml = self.sh(&["shell", "cat", "/sdcard/window_dump.xml"])?;
        snapshot::parse(&xml)
    }

    async fn resolve(&self, loc: &Locator) -> Result<(i64, i64)> {
        let center = |f: &agent_controller_core::Rect| {
            let (x, y) = f.center();
            (x as i64, y as i64)
        };
        match loc {
            Locator::Point { x, y } => Ok((*x as i64, *y as i64)),
            Locator::Css(_) => Err(anyhow!("CSS selectors are not supported by android-emu")),
            Locator::Ref(r) => {
                let snap = snapshot::load_cache(&self.paths.refs)?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| &e.r#ref == r)
                    .ok_or_else(|| anyhow!("no cached element @{r}; run `snapshot` first"))?;
                Ok(center(&el.frame))
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
                Ok(center(&el.frame))
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
                                Some(nm) => e
                                    .label
                                    .as_deref()
                                    .map(|l| l.to_lowercase().contains(&nm.to_lowercase()))
                                    .unwrap_or(false),
                            }
                    })
                    .ok_or_else(|| anyhow!("no {role} matching {name:?}"))?;
                Ok(center(&el.frame))
            }
        }
    }

    fn screen_size(&self) -> Result<(i64, i64)> {
        // "Physical size: 1080x2400" (and maybe an "Override size:" line).
        let out = self.sh(&["shell", "wm", "size"])?;
        let line = out
            .lines()
            .find(|l| l.contains("Override size"))
            .or_else(|| out.lines().find(|l| l.contains("Physical size")))
            .ok_or_else(|| anyhow!("could not read screen size"))?;
        let dims = line.split(':').nth(1).unwrap_or("").trim();
        let (w, h) = dims.split_once('x').ok_or_else(|| anyhow!("bad size {dims:?}"))?;
        Ok((w.trim().parse()?, h.trim().parse()?))
    }
}

#[async_trait]
impl Controller for AndroidEmuController {
    async fn navigate(&self, target: &str) -> Result<()> {
        if target.contains("://") {
            self.sh(&[
                "shell",
                "am",
                "start",
                "-a",
                "android.intent.action.VIEW",
                "-d",
                target,
            ])?;
        } else if target.eq_ignore_ascii_case("home") {
            self.sh(&["shell", "input", "keyevent", "KEYCODE_HOME"])?;
        } else if target.contains('/') {
            // package/activity
            self.sh(&["shell", "am", "start", "-n", target])?;
        } else {
            // package name → launch its launcher activity
            self.sh(&["shell", "monkey", "-p", target, "-c", "android.intent.category.LAUNCHER", "1"])?;
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
        self.sh(&["shell", "input", "tap", &x.to_string(), &y.to_string()])?;
        Ok(())
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        // `input text` treats spaces specially; %s encodes a space.
        let encoded = text.replace(' ', "%s");
        self.sh(&["shell", "input", "text", &encoded])?;
        Ok(())
    }

    async fn press(&self, key: &str) -> Result<()> {
        let code = keyevent(key).ok_or_else(|| anyhow!("unknown key: {key}"))?;
        self.sh(&["shell", "input", "keyevent", code])?;
        Ok(())
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let (w, h) = self.screen_size()?;
        let (cx, cy) = (w / 2, h / 2);
        let d = (amount.max(1) as i64) / 2;
        // Finger moves opposite to the content direction.
        let (x1, y1, x2, y2) = match dir {
            ScrollDir::Down => (cx, cy + d, cx, cy - d),
            ScrollDir::Up => (cx, cy - d, cx, cy + d),
            ScrollDir::Left => (cx + d, cy, cx - d, cy),
            ScrollDir::Right => (cx - d, cy, cx + d, cy),
        };
        self.sh(&[
            "shell", "input", "swipe",
            &x1.to_string(), &y1.to_string(), &x2.to_string(), &y2.to_string(), "300",
        ])?;
        Ok(())
    }

    async fn screenshot(&self) -> Result<Image> {
        let data = adb::adb_bytes(Some(&self.serial), &["exec-out", "screencap", "-p"])?;
        Ok(Image {
            data,
            format: "png".into(),
        })
    }

    fn backend(&self) -> Backend {
        Backend::AndroidEmu
    }

    fn target(&self) -> String {
        self.serial.clone()
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

fn keyevent(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => "KEYCODE_ENTER",
        "back" => "KEYCODE_BACK",
        "home" => "KEYCODE_HOME",
        "tab" => "KEYCODE_TAB",
        "space" => "KEYCODE_SPACE",
        "delete" | "backspace" => "KEYCODE_DEL",
        "escape" | "esc" => "KEYCODE_ESCAPE",
        "up" => "KEYCODE_DPAD_UP",
        "down" => "KEYCODE_DPAD_DOWN",
        "left" => "KEYCODE_DPAD_LEFT",
        "right" => "KEYCODE_DPAD_RIGHT",
        "menu" => "KEYCODE_MENU",
        "power" => "KEYCODE_POWER",
        _ => return None,
    })
}

/// Session-aware entry point for the Android emulator backend.
pub struct AndroidEmuFactory;

#[async_trait]
impl BackendFactory for AndroidEmuFactory {
    fn backend(&self) -> Backend {
        Backend::AndroidEmu
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let serial = match &opts.session {
            Some(s) => s.clone(),
            None => adb::single_serial()?,
        };
        Ok(Identity {
            id: format!("android-emu/{serial}"),
            target: serial,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        _opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        let serial = rec.target.clone();
        rec.runtime.alive = true;
        rec.config = serde_json::json!({ "serial": serial });
        let paths = store.paths(&rec.id)?;
        Ok(Box::new(AndroidEmuController { serial, paths }))
    }
}
