//! User configuration: browser launch settings (window geometry, headless,
//! extra args), layered from `<home>/config.toml` → environment → per-call
//! overrides (CLI flags / MCP args). Backends read the resolved
//! [`LaunchConfig`] off [`crate::Options::launch`] when they cold-start a browser.
//!
//! Window size is opinionated: with nothing configured, browsers open at
//! [`DEFAULT_WIDTH`]×[`DEFAULT_HEIGHT`] — a [`DEFAULT_ASPECT`] (≈1.7:1)
//! landscape window. Configure only one dimension and the other is derived at
//! that same ratio, so the aspect holds unless both are set explicitly.

use crate::{anyhow, Backend, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Width : height ratio used to fill in a missing dimension.
pub const DEFAULT_ASPECT: f64 = 1.7;
/// Default window width in logical pixels.
pub const DEFAULT_WIDTH: u32 = 1360;
/// Default window height in logical pixels (`DEFAULT_WIDTH / DEFAULT_ASPECT`).
pub const DEFAULT_HEIGHT: u32 = 800;

/// File name of the user config, relative to the session-store home.
pub const CONFIG_FILE: &str = "config.toml";

/// Env var: `WxH` (or `W,H`) window size, e.g. `AGENT_CONTROLLER_WINDOW_SIZE=1360x800`.
pub const ENV_WINDOW_SIZE: &str = "AGENT_CONTROLLER_WINDOW_SIZE";
/// Env var: `X,Y` window position, e.g. `AGENT_CONTROLLER_WINDOW_POSITION=0,0`.
pub const ENV_WINDOW_POSITION: &str = "AGENT_CONTROLLER_WINDOW_POSITION";
/// Env var: `1`/`true` to launch headless (chrome/firefox).
pub const ENV_HEADLESS: &str = "AGENT_CONTROLLER_HEADLESS";

/// How a browser window is created on cold start. Every field is optional so
/// layers can be merged; call [`LaunchConfig::window_size`] for the effective
/// geometry. Applies to the browser backends (chrome/firefox/safari); other
/// backends ignore it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchConfig {
    /// Window width in logical pixels.
    pub width: Option<u32>,
    /// Window height in logical pixels.
    pub height: Option<u32>,
    /// Window left edge (chrome/safari; firefox has no launch-time position).
    pub x: Option<i32>,
    /// Window top edge (chrome/safari).
    pub y: Option<i32>,
    /// Launch without a visible window (chrome/firefox; safari cannot).
    pub headless: Option<bool>,
    /// Extra command-line arguments appended to the browser launch
    /// (chrome/firefox). Layers concatenate rather than replace.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl LaunchConfig {
    /// Layer `over` on top of `self`: set fields in `over` win, `args` append.
    pub fn merge(&self, over: &LaunchConfig) -> LaunchConfig {
        let mut args = self.args.clone();
        args.extend(over.args.iter().cloned());
        LaunchConfig {
            width: over.width.or(self.width),
            height: over.height.or(self.height),
            x: over.x.or(self.x),
            y: over.y.or(self.y),
            headless: over.headless.or(self.headless),
            args,
        }
    }

    /// Effective `(width, height)`: both when set; one derives the other at
    /// [`DEFAULT_ASPECT`]; neither → the defaults.
    pub fn window_size(&self) -> (u32, u32) {
        match (self.width, self.height) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, ((w as f64) / DEFAULT_ASPECT).round().max(1.0) as u32),
            (None, Some(h)) => (((h as f64) * DEFAULT_ASPECT).round().max(1.0) as u32, h),
            (None, None) => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        }
    }

    /// Effective position, if any was configured.
    pub fn window_position(&self) -> Option<(i32, i32)> {
        match (self.x, self.y) {
            (None, None) => None,
            (x, y) => Some((x.unwrap_or(0), y.unwrap_or(0))),
        }
    }

    pub fn is_headless(&self) -> bool {
        self.headless.unwrap_or(false)
    }

    /// Reject geometry a browser can't open (zero-sized windows).
    pub fn validate(&self) -> Result<()> {
        if self.width == Some(0) || self.height == Some(0) {
            return Err(anyhow!("launch window size must be non-zero"));
        }
        Ok(())
    }

    /// Parse `WxH` / `W,H` / `W×H` (whitespace tolerated) into `(width, height)`.
    pub fn parse_size(s: &str) -> Result<(u32, u32)> {
        let norm = s.trim().to_ascii_lowercase();
        let parts: Vec<&str> = norm
            .split(|c| c == 'x' || c == ',' || c == '×' || c == '*')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let bad = || anyhow!("invalid window size {s:?}: expected WIDTHxHEIGHT, e.g. 1360x800");
        if parts.len() != 2 {
            return Err(bad());
        }
        let w: u32 = parts[0].parse().map_err(|_| bad())?;
        let h: u32 = parts[1].parse().map_err(|_| bad())?;
        if w == 0 || h == 0 {
            return Err(anyhow!(
                "invalid window size {s:?}: dimensions must be non-zero"
            ));
        }
        Ok((w, h))
    }

    /// Parse `X,Y` (whitespace tolerated) into `(x, y)`.
    pub fn parse_position(s: &str) -> Result<(i32, i32)> {
        let bad = || anyhow!("invalid window position {s:?}: expected X,Y, e.g. 0,0");
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 2 {
            return Err(bad());
        }
        Ok((
            parts[0].parse().map_err(|_| bad())?,
            parts[1].parse().map_err(|_| bad())?,
        ))
    }

    /// The layer contributed by environment variables
    /// ([`ENV_WINDOW_SIZE`], [`ENV_WINDOW_POSITION`], [`ENV_HEADLESS`]).
    pub fn from_env() -> Result<LaunchConfig> {
        let mut c = LaunchConfig::default();
        if let Ok(v) = std::env::var(ENV_WINDOW_SIZE) {
            if !v.trim().is_empty() {
                let (w, h) = Self::parse_size(&v).map_err(|e| anyhow!("{ENV_WINDOW_SIZE}: {e}"))?;
                c.width = Some(w);
                c.height = Some(h);
            }
        }
        if let Ok(v) = std::env::var(ENV_WINDOW_POSITION) {
            if !v.trim().is_empty() {
                let (x, y) =
                    Self::parse_position(&v).map_err(|e| anyhow!("{ENV_WINDOW_POSITION}: {e}"))?;
                c.x = Some(x);
                c.y = Some(y);
            }
        }
        if let Ok(v) = std::env::var(ENV_HEADLESS) {
            c.headless = Some(matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ));
        }
        Ok(c)
    }

    /// Resolve the effective launch config for `backend`: config file
    /// (`[launch]` then `[launch.<backend>]`) → environment → `overrides`
    /// (CLI flags / MCP args, highest precedence).
    pub fn resolve(
        home: &Path,
        backend: Backend,
        overrides: &LaunchConfig,
    ) -> Result<LaunchConfig> {
        let file = ConfigFile::load(home)?;
        let resolved = file
            .launch_for(backend)
            .merge(&Self::from_env()?)
            .merge(overrides);
        resolved.validate()?;
        Ok(resolved)
    }
}

/// The `[launch]` table: defaults for every browser plus per-backend overrides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchSection {
    #[serde(flatten)]
    pub all: LaunchConfig,
    pub chrome: LaunchConfig,
    pub firefox: LaunchConfig,
    pub safari: LaunchConfig,
}

/// Contents of `<home>/config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigFile {
    pub launch: LaunchSection,
}

impl ConfigFile {
    /// Where the config file lives for a store rooted at `home`.
    pub fn path(home: &Path) -> PathBuf {
        home.join(CONFIG_FILE)
    }

    /// Load `<home>/config.toml`; a missing file is the default config, a
    /// malformed one is an error naming the path.
    pub fn load(home: &Path) -> Result<ConfigFile> {
        let path = Self::path(home);
        match std::fs::read_to_string(&path) {
            Ok(s) => Self::parse(&s).map_err(|e| anyhow!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
            Err(e) => Err(anyhow!("reading {}: {e}", path.display())),
        }
    }

    pub fn parse(s: &str) -> Result<ConfigFile> {
        toml::from_str(s).map_err(|e| anyhow!("{}", e.message()))
    }

    /// `[launch]` merged with `[launch.<backend>]`. Non-browser backends get
    /// the shared `[launch]` values only (they ignore them anyway).
    pub fn launch_for(&self, backend: Backend) -> LaunchConfig {
        let per = match backend {
            Backend::Chrome => &self.launch.chrome,
            Backend::Firefox => &self.launch.firefox,
            Backend::Safari => &self.launch.safari,
            _ => return self.launch.all.clone(),
        };
        self.launch.all.merge(per)
    }

    /// A commented starter file showing every launch key at its default.
    pub fn template() -> String {
        format!(
            r#"# agent-controller configuration.
# Browser launch settings apply when a browser is cold-started (chrome, firefox,
# safari). Precedence: [launch.<backend>] > [launch] > built-in defaults; the
# {ENV_WINDOW_SIZE} / {ENV_WINDOW_POSITION} / {ENV_HEADLESS}
# env vars and the --window-size / --window-position / --headless flags override
# this file.

[launch]
# Window size in logical pixels. Default {w}x{h} (a {aspect}:1 landscape window).
# Set only one and the other is derived at that same ratio.
width = {w}
height = {h}
# Window position (chrome/safari; firefox ignores it).
# x = 0
# y = 0
# Launch without a visible window (chrome/firefox; safari cannot).
# headless = false
# Extra browser command-line arguments (chrome/firefox).
# args = []

# Per-backend overrides use the same keys:
# [launch.chrome]
# args = ["--disable-gpu"]
# [launch.firefox]
# headless = true
# [launch.safari]
# width = 1200
"#,
            w = DEFAULT_WIDTH,
            h = DEFAULT_HEIGHT,
            aspect = DEFAULT_ASPECT,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_landscape_at_aspect() {
        let (w, h) = LaunchConfig::default().window_size();
        assert_eq!((w, h), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        assert!(((w as f64 / h as f64) - DEFAULT_ASPECT).abs() < 0.01);
    }

    #[test]
    fn missing_dimension_is_derived_at_aspect() {
        let c = LaunchConfig {
            width: Some(1700),
            ..Default::default()
        };
        assert_eq!(c.window_size(), (1700, 1000));
        let c = LaunchConfig {
            height: Some(1000),
            ..Default::default()
        };
        assert_eq!(c.window_size(), (1700, 1000));
        let c = LaunchConfig {
            width: Some(1000),
            height: Some(1000),
            ..Default::default()
        };
        assert_eq!(c.window_size(), (1000, 1000));
    }

    #[test]
    fn merge_prefers_overlay_and_appends_args() {
        let base = LaunchConfig {
            width: Some(1000),
            height: Some(500),
            headless: Some(false),
            args: vec!["--a".into()],
            ..Default::default()
        };
        let over = LaunchConfig {
            width: Some(1360),
            headless: Some(true),
            args: vec!["--b".into()],
            ..Default::default()
        };
        let m = base.merge(&over);
        assert_eq!(m.width, Some(1360));
        assert_eq!(m.height, Some(500));
        assert_eq!(m.headless, Some(true));
        assert_eq!(m.args, vec!["--a", "--b"]);
    }

    #[test]
    fn parses_sizes_and_positions() {
        assert_eq!(LaunchConfig::parse_size("1360x800").unwrap(), (1360, 800));
        assert_eq!(
            LaunchConfig::parse_size(" 1360 , 800 ").unwrap(),
            (1360, 800)
        );
        assert_eq!(LaunchConfig::parse_size("1360X800").unwrap(), (1360, 800));
        assert!(LaunchConfig::parse_size("1360").is_err());
        assert!(LaunchConfig::parse_size("0x800").is_err());
        assert!(LaunchConfig::parse_size("wide").is_err());
        assert_eq!(LaunchConfig::parse_position("10,-20").unwrap(), (10, -20));
        assert!(LaunchConfig::parse_position("10").is_err());
    }

    #[test]
    fn toml_layers_per_backend_over_shared() {
        let f = ConfigFile::parse(
            r#"
[launch]
width = 1500
headless = false
args = ["--shared"]

[launch.chrome]
height = 600
args = ["--chrome-only"]

[launch.firefox]
headless = true
"#,
        )
        .unwrap();
        let chrome = f.launch_for(Backend::Chrome);
        assert_eq!(chrome.window_size(), (1500, 600));
        assert_eq!(chrome.headless, Some(false));
        assert_eq!(chrome.args, vec!["--shared", "--chrome-only"]);
        let firefox = f.launch_for(Backend::Firefox);
        assert_eq!(firefox.window_size(), (1500, 882));
        assert_eq!(firefox.headless, Some(true));
        assert_eq!(firefox.args, vec!["--shared"]);
        let safari = f.launch_for(Backend::Safari);
        assert_eq!(safari.window_size(), (1500, 882));
        // Non-browser backends get the shared values only.
        assert_eq!(f.launch_for(Backend::Mac).width, Some(1500));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(ConfigFile::parse("[launch]\nwidht = 100\n").is_err());
        assert!(ConfigFile::parse("[lunch]\nwidth = 100\n").is_err());
    }

    #[test]
    fn template_parses_to_defaults() {
        let f = ConfigFile::parse(&ConfigFile::template()).unwrap();
        assert_eq!(
            f.launch_for(Backend::Chrome).window_size(),
            (DEFAULT_WIDTH, DEFAULT_HEIGHT)
        );
    }

    #[test]
    fn empty_and_missing_files() {
        assert_eq!(ConfigFile::parse("").unwrap(), ConfigFile::default());
        let dir = std::env::temp_dir().join(format!("ac-config-test-{}", std::process::id()));
        assert_eq!(ConfigFile::load(&dir).unwrap(), ConfigFile::default());
    }
}
