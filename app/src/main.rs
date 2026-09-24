//! `agent-controller` CLI. Backend-agnostic surface over `Box<dyn Controller>`,
//! with per-item persistent sessions. Currently wires the iOS Simulator backend;
//! other backends plug in via the factory registry.

mod factory;
mod guidance;
mod mcp;

use agent_controller_core::{now, Backend, ConfigFile, LaunchConfig, Locator, ScrollDir, SessionStore};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Semantic version from the workspace `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Git commit (short sha, `-dirty` if the tree had changes) captured by `build.rs`.
pub const GIT_DESC: &str = env!("AGENT_CONTROLLER_GIT_DESC");
/// Build date (UTC, `YYYY-MM-DD`) captured by `build.rs`.
pub const BUILD_DATE: &str = env!("AGENT_CONTROLLER_BUILD_DATE");
/// `0.2.0 (abc123def, 2026-09-24)` — what `--version` and `version` print.
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AGENT_CONTROLLER_GIT_DESC"),
    ", ",
    env!("AGENT_CONTROLLER_BUILD_DATE"),
    ")"
);

#[derive(Parser)]
#[command(
    name = "agent-controller",
    about = "Drive apps/devices for agents",
    version = LONG_VERSION
)]
struct Cli {
    /// Backend to use.
    #[arg(long, default_value = "ios-sim", global = true)]
    backend: String,

    /// iOS Simulator UDID (defaults to the single booted sim).
    #[arg(long, global = true)]
    udid: Option<String>,

    /// mac app bundle id / name.
    #[arg(long, global = true)]
    app: Option<String>,

    /// Instance name for multi-instance backends (browsers).
    #[arg(long, global = true)]
    session: Option<String>,

    /// Bind to a specific OS process by pid (desktop backends: mac, windows),
    /// to pick one instance among several of the same app.
    #[arg(long, global = true)]
    pid: Option<i32>,

    /// Session store home (overrides discovery).
    #[arg(long, global = true)]
    home: Option<PathBuf>,

    /// Use the global cursor / focus-stealing input path (mac) for UI that
    /// ignores targeted events. Default is non-interruptive (CGEventPostToPid).
    #[arg(long, global = true)]
    takeover: bool,

    /// Browser window size on cold start, WIDTHxHEIGHT (e.g. 1360x800).
    /// Overrides config.toml / env. Default 1360x800 (~1.7:1).
    #[arg(long, global = true, value_name = "WxH")]
    window_size: Option<String>,

    /// Browser window position on cold start, X,Y (chrome/safari).
    #[arg(long, global = true, value_name = "X,Y")]
    window_position: Option<String>,

    /// Launch the browser headless (chrome/firefox).
    #[arg(long, global = true)]
    headless: bool,

    /// Emit structured JSON instead of human text.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open a target: a URL, a bundle id to launch, or `home`.
    Use { target: String },
    /// Print the accessibility tree with @refs.
    Snapshot,
    /// Click/tap an element: @ref, "Label", or (x,y).
    Click { locator: String },
    /// Type text into the focused element.
    Type { text: String },
    /// Press a named key or hardware button (Enter, Home, Lock, ...).
    Press { key: String },
    /// Scroll/swipe: up|down|left|right [amount-in-points].
    Scroll {
        direction: String,
        #[arg(default_value_t = 400)]
        amount: i32,
    },
    /// Capture a screenshot (defaults into the session's screenshots/ dir).
    Screenshot { path: Option<String> },
    /// Invoke a native menu by path, e.g. "Format>Font>Bold" (mac).
    Menu { path: String },
    /// Attach file(s) to a file input (browsers): upload <locator> <path>...
    Upload {
        locator: String,
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Show the bound target and its capabilities.
    Status,
    /// List displays with bounds + backing scale (for pixel↔point mapping).
    Displays,
    /// Check macOS permissions and guide through granting them.
    Doctor,
    /// Run an MCP server (stdio) exposing the backends as tools.
    Mcp,
    /// List all sessions.
    Sessions,
    /// Manage sessions.
    #[command(subcommand)]
    Session(SessionCmd),
    /// Show or initialise the user config (browser launch settings).
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Print the version, git commit, build date, and platform (`--json` for a record).
    Version,
    /// Internal: run the firefox BiDi daemon (not for direct use).
    #[command(name = "__firefox-daemon", hide = true)]
    FirefoxDaemon {
        #[arg(long)]
        session: String,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        addr: String,
        /// Resolved launch config as JSON (from the spawning CLI).
        #[arg(long)]
        launch: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the config file path and the effective launch settings per browser
    /// (file → env → flags).
    Show,
    /// Write a commented config.toml template (refuses to overwrite without --force).
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Print the config file path.
    Path,
}

#[derive(Subcommand)]
enum SessionCmd {
    /// Show a session record.
    Show { id: String },
    /// Remove a session (record + artifacts).
    Rm {
        id: String,
        #[arg(long)]
        force: bool,
    },
    /// Print a session's directory.
    Path { id: String },
    /// Drop sessions whose processes are gone or older than --max-age days.
    Prune {
        #[arg(long)]
        max_age_days: Option<u64>,
    },
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = SessionStore::discover(cli.home.clone())?;

    // These commands don't need a controller.
    match &cli.command {
        Command::Sessions => return list_sessions(&store, cli.json),
        Command::Session(sub) => return session_cmd(&store, sub, cli.json),
        Command::Config(sub) => return config_cmd(&store, sub, &cli),
        Command::Version => return version_cmd(cli.json),
        #[cfg(target_os = "macos")]
        Command::Displays => {
            let displays = agent_controller_mac::displays();
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&displays)?);
            } else {
                for d in displays {
                    println!(
                        "display {}{}  origin=({:.0},{:.0}) size={:.0}x{:.0} pts  scale={}x",
                        d.id,
                        if d.main { " (main)" } else { "" },
                        d.x,
                        d.y,
                        d.width,
                        d.height,
                        d.scale
                    );
                }
            }
            return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        Command::Displays => return Err(anyhow::anyhow!("`displays` is macOS-only")),
        Command::Doctor => return doctor(cli.json).await,
        Command::Mcp => return mcp::serve().await,
        Command::FirefoxDaemon {
            session,
            profile,
            addr,
            launch,
        } => {
            let launch: LaunchConfig = match launch {
                Some(j) => serde_json::from_str(j)?,
                None => LaunchConfig::default(),
            };
            return agent_controller_firefox::run_daemon(
                session.clone(),
                profile.clone(),
                addr.clone(),
                launch,
            )
            .await;
        }
        _ => {}
    }

    let backend: Backend = cli.backend.parse()?;
    let opts = agent_controller_core::Options {
        udid: cli.udid.clone(),
        app: cli.app.clone(),
        session: cli.session.clone(),
        pid: cli.pid,
        takeover: cli.takeover,
        launch: launch_overrides(&cli)?,
    };
    let (ctrl, id) = factory::create(&store, backend, opts).await?;

    match cli.command {
        Command::Use { target } => {
            ctrl.navigate(&target).await?;
            ok(cli.json, &format!("opened {target}"));
        }
        Command::Snapshot => {
            let snap = ctrl.snapshot().await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                print!("{}", snap.render());
            }
        }
        Command::Click { locator } => {
            let loc = Locator::parse(&locator);
            ctrl.click(&loc).await?;
            ok(cli.json, &format!("clicked {locator}"));
        }
        Command::Type { text } => {
            ctrl.type_text(&text).await?;
            ok(cli.json, &format!("typed {} chars", text.chars().count()));
        }
        Command::Press { key } => {
            ctrl.press(&key).await?;
            ok(cli.json, &format!("pressed {key}"));
        }
        Command::Scroll { direction, amount } => {
            let dir: ScrollDir = direction.parse()?;
            ctrl.scroll(dir, amount).await?;
            ok(cli.json, &format!("scrolled {direction} {amount}"));
        }
        Command::Screenshot { path } => {
            let img = ctrl.screenshot().await?;
            let out = match path {
                Some(p) => PathBuf::from(p),
                None => store
                    .paths(&id)?
                    .screenshots
                    .join(format!("{}.{}", now_millis(), img.format)),
            };
            std::fs::write(&out, &img.data)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"ok": true, "path": out, "format": img.format, "bytes": img.data.len()})
                );
            } else {
                println!(
                    "wrote {} ({} bytes, {})",
                    out.display(),
                    img.data.len(),
                    img.format
                );
            }
        }
        Command::Menu { path } => {
            ctrl.menu(&path).await?;
            ok(cli.json, &format!("invoked menu {path}"));
        }
        Command::Upload { locator, paths } => {
            let loc = Locator::parse(&locator);
            // Resolve to absolute paths the browser process can read (canonicalize
            // also verifies each file exists before we hand it to the backend).
            let abs: Vec<String> = paths
                .iter()
                .map(|p| {
                    std::fs::canonicalize(p)
                        .map(|c| c.to_string_lossy().into_owned())
                        .map_err(|e| anyhow::anyhow!("{p}: {e}"))
                })
                .collect::<Result<_>>()?;
            ctrl.set_files(&loc, &abs).await?;
            ok(cli.json, &format!("uploaded {} file(s) to {locator}", abs.len()));
        }
        Command::Status => {
            let caps = ctrl.capabilities();
            let backend = ctrl.backend();
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"backend": backend.as_str(), "session": id, "target": ctrl.target(), "capabilities": caps, "version": VERSION})
                );
            } else {
                println!(
                    "backend: {}\nsession: {}\ntarget: {}\ncapabilities: {:?}",
                    backend.as_str(),
                    id,
                    ctrl.target(),
                    caps
                );
            }
        }
        Command::Sessions
        | Command::Session(_)
        | Command::Config(_)
        | Command::Version
        | Command::Displays
        | Command::Doctor
        | Command::Mcp
        | Command::FirefoxDaemon { .. } => {
            unreachable!("handled above")
        }
    }
    Ok(())
}

/// Build provenance as a JSON record (also the shape `version --json` prints).
pub fn version_info() -> serde_json::Value {
    serde_json::json!({
        "name": "agent-controller",
        "version": VERSION,
        "git": GIT_DESC,
        "build_date": BUILD_DATE,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

fn version_cmd(json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&version_info())?);
    } else {
        println!(
            "agent-controller {LONG_VERSION} {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }
    Ok(())
}

/// The launch-config layer contributed by this invocation's flags.
fn launch_overrides(cli: &Cli) -> Result<LaunchConfig> {
    let mut l = LaunchConfig::default();
    if let Some(s) = &cli.window_size {
        let (w, h) = LaunchConfig::parse_size(s)?;
        l.width = Some(w);
        l.height = Some(h);
    }
    if let Some(s) = &cli.window_position {
        let (x, y) = LaunchConfig::parse_position(s)?;
        l.x = Some(x);
        l.y = Some(y);
    }
    if cli.headless {
        l.headless = Some(true);
    }
    Ok(l)
}

fn config_cmd(store: &SessionStore, sub: &ConfigCmd, cli: &Cli) -> Result<()> {
    let path = ConfigFile::path(store.home());
    match sub {
        ConfigCmd::Path => println!("{}", path.display()),
        ConfigCmd::Init { force } => {
            if path.exists() && !force {
                return Err(anyhow::anyhow!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                ));
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, ConfigFile::template())?;
            ok(cli.json, &format!("wrote {}", path.display()));
        }
        ConfigCmd::Show => {
            // Validate the file up front so a typo is reported, not masked.
            ConfigFile::load(store.home())?;
            let overrides = launch_overrides(cli)?;
            let browsers = [Backend::Chrome, Backend::Firefox, Backend::Safari];
            if cli.json {
                let mut effective = serde_json::Map::new();
                for b in browsers {
                    let l = LaunchConfig::resolve(store.home(), b, &overrides)?;
                    let (w, h) = l.window_size();
                    let mut v = serde_json::to_value(&l)?;
                    v["effective_width"] = serde_json::json!(w);
                    v["effective_height"] = serde_json::json!(h);
                    effective.insert(b.as_str().to_string(), v);
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": path,
                        "exists": path.exists(),
                        "launch": effective,
                    }))?
                );
                return Ok(());
            }
            println!(
                "config: {} ({})",
                path.display(),
                if path.exists() { "present" } else { "absent — using defaults; `config init` to create" }
            );
            println!("effective launch settings (file → env → flags):");
            for b in browsers {
                let l = LaunchConfig::resolve(store.home(), b, &overrides)?;
                let (w, h) = l.window_size();
                let pos = l
                    .window_position()
                    .map(|(x, y)| format!("  position={x},{y}"))
                    .unwrap_or_default();
                let args = if l.args.is_empty() {
                    String::new()
                } else {
                    format!("  args={:?}", l.args)
                };
                println!(
                    "  {:<8} window={w}x{h} ({:.2}:1){pos}  headless={}{args}",
                    b.as_str(),
                    w as f64 / h as f64,
                    l.is_headless()
                );
            }
        }
    }
    Ok(())
}

fn list_sessions(store: &SessionStore, json: bool) -> Result<()> {
    let sessions = store.list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        println!("no sessions (store: {})", store.home().display());
        return Ok(());
    }
    println!("{:<32} {:<8} {:<24} ago", "SESSION", "ALIVE", "TARGET");
    let t = now();
    for s in sessions {
        println!(
            "{:<32} {:<8} {:<24} {}s",
            s.id,
            s.runtime.alive,
            truncate(&s.target, 24),
            t.saturating_sub(s.last_used_at)
        );
    }
    Ok(())
}

fn session_cmd(store: &SessionStore, sub: &SessionCmd, json: bool) -> Result<()> {
    match sub {
        SessionCmd::Show { id } => match store.load(id)? {
            Some(rec) => println!("{}", serde_json::to_string_pretty(&rec)?),
            None => return Err(anyhow::anyhow!("no such session: {id}")),
        },
        SessionCmd::Path { id } => {
            println!("{}", store.paths(id)?.dir.display());
        }
        SessionCmd::Rm { id, force } => {
            if !force && store.load(id)?.is_none() {
                return Err(anyhow::anyhow!("no such session: {id}"));
            }
            store.remove(id)?;
            ok(json, &format!("removed {id}"));
        }
        SessionCmd::Prune { max_age_days } => {
            let t = now();
            let mut removed = 0;
            for s in store.list()? {
                let stale = !s.runtime.alive
                    || max_age_days
                        .map(|d| t.saturating_sub(s.last_used_at) > d * 86_400)
                        .unwrap_or(false);
                if stale {
                    store.remove(&s.id)?;
                    removed += 1;
                }
            }
            ok(json, &format!("pruned {removed} session(s)"));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn doctor(_json: bool) -> Result<()> {
    println!("doctor: macOS-only permission checks; nothing to verify on this platform.");
    println!("Browser backends (firefox/chrome) and android-emu need their tools on PATH.");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn doctor(json: bool) -> Result<()> {
    use agent_controller_mac::setup;
    let ax = setup::ax_trusted();
    let screen = setup::screen_recording_ok();
    // Safari probe briefly opens/closes a Safari window if Remote Automation is on.
    let (safaridriver, remote_automation) =
        agent_controller_safari::remote_automation_status().await;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "accessibility": ax,
                "screen_recording": screen,
                "safaridriver": safaridriver,
                "safari_remote_automation": remote_automation,
                "ready": ax && screen,
            })
        );
        return Ok(());
    }

    let mark = |b: bool| if b { "granted ✓" } else { "NOT granted ✗" };
    println!("agent-controller doctor — macOS permissions\n");
    println!("  Accessibility (mac)          : {}", mark(ax));
    println!("  Screen Recording (mac)       : {}", mark(screen));
    println!(
        "  Safari Remote Automation     : {}",
        if !safaridriver {
            "safaridriver not found"
        } else if remote_automation {
            "enabled ✓"
        } else {
            "NOT enabled ✗"
        }
    );
    println!();

    if !ax {
        println!("• Accessibility (required to read the UI tree and send input)");
        println!("  Triggering the system prompt and opening the settings pane…");
        setup::ax_request();
        open_settings("Privacy_Accessibility");
        println!("  Enable your terminal app (e.g. iTerm) there, then re-run `agent-controller doctor`.\n");
    }
    if !screen {
        println!("• Screen Recording (required for mac screenshots)");
        println!("  Triggering the prompt and opening the settings pane…");
        setup::screen_recording_request();
        open_settings("Privacy_ScreenCapture");
        println!("  Enable your terminal app there, then re-run `agent-controller doctor`.\n");
    }
    if safaridriver && !remote_automation {
        println!("• Safari Remote Automation (required for the safari backend)");
        println!("  Run: safaridriver --enable   (authenticate when prompted)");
        println!("  or Safari ▸ Settings ▸ Advanced ▸ \"Show features for web developers\",");
        println!("  then Develop ▸ Allow Remote Automation. Re-run `agent-controller doctor`.\n");
    }

    if ax && screen && (!safaridriver || remote_automation) {
        println!("All set.");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_settings(anchor: &str) {
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    let _ = std::process::Command::new("open").arg(url).status();
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n.saturating_sub(1)])
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn ok(json: bool, msg: &str) {
    if json {
        println!("{}", serde_json::json!({"ok": true, "message": msg}));
    } else {
        println!("{msg}");
    }
}
