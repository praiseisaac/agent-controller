//! Backend-agnostic core for `agent-controller`.
//!
//! Defines the [`Controller`] trait that every backend (mac, firefox, chrome,
//! safari, ios-sim) implements, plus the shared value types the CLI / MCP / chat
//! layers speak in. A backend is selected at runtime by the factory; everything
//! above the trait is written once and works for all backends.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod session;
pub use session::{now, SessionPaths, SessionRecord, SessionStore};

pub use anyhow::{anyhow, Error, Result};

/// Which concrete backend a session drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Mac,
    Windows,
    Firefox,
    Chrome,
    Safari,
    IosSim,
    AndroidEmu,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Mac => "mac",
            Backend::Windows => "windows",
            Backend::Firefox => "firefox",
            Backend::Chrome => "chrome",
            Backend::Safari => "safari",
            Backend::IosSim => "ios-sim",
            Backend::AndroidEmu => "android-emu",
        }
    }
}

impl std::str::FromStr for Backend {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "mac" | "macos" => Backend::Mac,
            "windows" | "win" => Backend::Windows,
            "firefox" => Backend::Firefox,
            "chrome" => Backend::Chrome,
            "safari" => Backend::Safari,
            "ios-sim" | "ios" | "iossim" | "simulator" => Backend::IosSim,
            "android-emu" | "android" | "androidemu" | "emulator" => Backend::AndroidEmu,
            other => return Err(anyhow!("unknown backend: {other}")),
        })
    }
}

/// A way to address an element. Backends interpret the variants they support and
/// return [`Error`] for the rest (advertised via [`Capabilities`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Locator {
    /// `@eN` reference produced by the most recent [`Controller::snapshot`].
    Ref(String),
    /// Accessibility label / accessible name, matched case-insensitively.
    Label(String),
    /// Visible text contained by the element.
    Text(String),
    /// Role plus optional accessible name, e.g. `AXButton` / `button`.
    Role { role: String, name: Option<String> },
    /// CSS selector (browser backends).
    Css(String),
    /// Raw on-screen point (logical points, origin top-left).
    Point { x: f64, y: f64 },
}

impl Locator {
    /// Parse a CLI token: `@e3` -> Ref, `css:#id` -> Css, `(120,40)` -> Point,
    /// anything else -> Label.
    pub fn parse(s: &str) -> Locator {
        if let Some(rest) = s.strip_prefix('@') {
            return Locator::Ref(rest.to_string());
        }
        if let Some(rest) = s.strip_prefix("css:") {
            return Locator::Css(rest.to_string());
        }
        if let Some(inner) = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            if let Some((x, y)) = inner.split_once(',') {
                if let (Ok(x), Ok(y)) = (x.trim().parse(), y.trim().parse()) {
                    return Locator::Point { x, y };
                }
            }
        }
        Locator::Label(s.to_string())
    }
}

/// A rectangle in logical screen points, origin top-left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// One node in a [`Snapshot`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    /// Stable `eN` reference within this snapshot.
    pub r#ref: String,
    pub role: Option<String>,
    pub label: Option<String>,
    pub value: Option<String>,
    pub frame: Rect,
    pub enabled: bool,
    /// Backend-specific action names available on the element (e.g. `AXPress`).
    #[serde(default)]
    pub actions: Vec<String>,
}

/// A point-in-time view of the target's interactive elements.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub elements: Vec<Element>,
}

impl Snapshot {
    /// Render an agent-readable outline (the format identical across backends).
    pub fn render(&self) -> String {
        let mut out = String::new();
        for e in &self.elements {
            let role = e.role.as_deref().unwrap_or("?");
            let label = e.label.as_deref().unwrap_or("");
            let mut line = format!("[@{}] {}", e.r#ref, role);
            if !label.is_empty() {
                line.push_str(&format!(" {label:?}"));
            }
            if let Some(v) = &e.value {
                if !v.is_empty() {
                    line.push_str(&format!(" value={v:?}"));
                }
            }
            if !e.frame.is_empty() {
                line.push_str(&format!(
                    "  ({:.0},{:.0} {:.0}x{:.0})",
                    e.frame.x, e.frame.y, e.frame.width, e.frame.height
                ));
            }
            if !e.enabled {
                line.push_str("  [disabled]");
            }
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

/// A captured image plus its container format (e.g. `png`).
#[derive(Debug, Clone)]
pub struct Image {
    pub data: Vec<u8>,
    pub format: String,
}

/// Scroll/swipe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

impl std::str::FromStr for ScrollDir {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "up" => ScrollDir::Up,
            "down" => ScrollDir::Down,
            "left" => ScrollDir::Left,
            "right" => ScrollDir::Right,
            other => return Err(anyhow!("unknown scroll direction: {other}")),
        })
    }
}

/// What a backend can do, so the dispatch/MCP layer can gate optional commands
/// instead of every backend faking unsupported ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Run arbitrary JS (browsers).
    pub eval: bool,
    /// Network interception / logs (browsers).
    pub network: bool,
    /// Invoke native menus (mac).
    pub menus: bool,
    /// Coordinate/synthetic input in addition to the structured tree.
    pub coordinates: bool,
    /// Capture screenshots.
    pub screenshot: bool,
    /// Attach files to a file `<input>` in-band ([`Controller::set_files`]).
    pub upload: bool,
}

/// Construction inputs common across backends; each uses what applies.
#[derive(Debug, Default, Clone)]
pub struct Options {
    /// iOS Simulator UDID (defaults to the single booted sim).
    pub udid: Option<String>,
    /// mac app bundle id / name.
    pub app: Option<String>,
    /// Instance name for multi-instance backends (browsers); default `default`.
    pub session: Option<String>,
    /// Bind to a specific OS process by pid, disambiguating among multiple
    /// instances of the same app (desktop backends: mac, windows).
    pub pid: Option<i32>,
    /// Use the global cursor / focus-stealing input path (mac) instead of the
    /// non-interruptive targeted path.
    pub takeover: bool,
}

/// The resolved identity of a session: its id plus the concrete target.
#[derive(Debug, Clone)]
pub struct Identity {
    /// `<backend>/<instance-key>`, e.g. `ios-sim/<udid>`.
    pub id: String,
    /// udid / url / bundle-id / app.
    pub target: String,
}

/// A backend's session entry point: derive a session id and open (resume or
/// cold-start) a [`Controller`] bound to it. Lives in each backend crate.
#[async_trait]
pub trait BackendFactory: Send + Sync {
    fn backend(&self) -> Backend;

    /// Compute the session identity from inputs WITHOUT spawning anything
    /// (e.g. resolve the booted UDID).
    async fn identify(&self, opts: &Options) -> Result<Identity>;

    /// Resume the live process if `rec.runtime` is reachable, else cold-start;
    /// update `rec.runtime`/`rec.config` accordingly. `opts` carries per-invocation
    /// flags (e.g. takeover).
    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        opts: &Options,
    ) -> Result<Box<dyn Controller>>;
}

/// The interface every backend inherits.
#[async_trait]
pub trait Controller: Send + Sync {
    /// Open a target: a URL for browsers, an app/bundle-id for mac/ios-sim.
    async fn navigate(&self, target: &str) -> Result<()>;

    /// Snapshot the current interactive elements with stable `@ref`s.
    async fn snapshot(&self) -> Result<Snapshot>;

    /// Click / tap the element addressed by `loc`.
    async fn click(&self, loc: &Locator) -> Result<()>;

    /// Type text into the focused element.
    async fn type_text(&self, text: &str) -> Result<()>;

    /// Press a named key or hardware button (e.g. `Enter`, `Home`).
    async fn press(&self, key: &str) -> Result<()>;

    /// Scroll/swipe in a direction by an approximate amount in points.
    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()>;

    /// Capture a screenshot of the target.
    async fn screenshot(&self) -> Result<Image>;

    /// Invoke a native menu by path (e.g. `Format>Font>Bold`). Backends without
    /// menus return an error.
    async fn menu(&self, _path: &str) -> Result<()> {
        Err(anyhow!("menu is not supported by this backend"))
    }

    /// Attach file(s) to a file `<input>` element (browsers), in-band — no native
    /// file dialog. `loc` must address the input (use `@ref` or `css:`); `paths`
    /// are absolute file paths. Backends without a DOM return an error.
    async fn set_files(&self, _loc: &Locator, _paths: &[String]) -> Result<()> {
        Err(anyhow!("file upload is not supported by this backend"))
    }

    /// Which backend this is.
    fn backend(&self) -> Backend;

    /// A human label for the bound target (a UDID, URL, or app id).
    fn target(&self) -> String;

    /// What this backend supports.
    fn capabilities(&self) -> Capabilities;
}
