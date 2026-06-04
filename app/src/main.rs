//! `agent-controller` CLI. Backend-agnostic surface over `Box<dyn Controller>`,
//! with per-item persistent sessions. Currently wires the iOS Simulator backend;
//! other backends plug in via the factory registry.

mod factory;
mod mcp;

use agent_controller_core::{now, Backend, Locator, ScrollDir, SessionStore};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "agent-controller", about = "Drive apps/devices for agents")]
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

    /// Session store home (overrides discovery).
    #[arg(long, global = true)]
    home: Option<PathBuf>,

    /// Use the global cursor / focus-stealing input path (mac) for UI that
    /// ignores targeted events. Default is non-interruptive (CGEventPostToPid).
    #[arg(long, global = true)]
    takeover: bool,

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
    /// Internal: run the firefox BiDi daemon (not for direct use).
    #[command(name = "__firefox-daemon", hide = true)]
    FirefoxDaemon {
        #[arg(long)]
        session: String,
        #[arg(long)]
        profile: PathBuf,
        #[arg(long)]
        addr: String,
    },
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
        } => {
            return agent_controller_firefox::run_daemon(
                session.clone(),
                profile.clone(),
                addr.clone(),
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
        takeover: cli.takeover,
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
        Command::Status => {
            let caps = ctrl.capabilities();
            let backend = ctrl.backend();
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"backend": backend.as_str(), "session": id, "target": ctrl.target(), "capabilities": caps})
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
        | Command::Displays
        | Command::Doctor
        | Command::Mcp
        | Command::FirefoxDaemon { .. } => {
            unreachable!("handled above")
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
