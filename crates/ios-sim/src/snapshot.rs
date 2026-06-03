//! Parse `idb`'s LEGACY accessibility JSON (a flat array) into a core [`Snapshot`]
//! with stable `@ref`s, and a tiny on-disk ref cache so `click @e3` works across
//! separate CLI invocations (no daemon needed).

use agent_controller_core::{Element, Rect, Result, Snapshot};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct RawFrame {
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
}

#[derive(Deserialize)]
struct RawEl {
    #[serde(rename = "AXLabel")]
    label: Option<String>,
    #[serde(rename = "AXValue")]
    value: Option<serde_json::Value>,
    role: Option<String>,
    enabled: Option<bool>,
    frame: Option<RawFrame>,
    #[serde(default)]
    custom_actions: Vec<String>,
}

fn value_to_string(v: Option<serde_json::Value>) -> Option<String> {
    match v {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
        Some(serde_json::Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

/// Parse the JSON returned by `accessibility_info` (LEGACY format).
pub fn parse(json: &str) -> Result<Snapshot> {
    let raws: Vec<RawEl> = serde_json::from_str(json)?;
    let mut elements = Vec::new();
    let mut n = 0usize;
    for r in raws {
        let frame = r
            .frame
            .map(|f| Rect {
                x: f.x,
                y: f.y,
                width: f.width,
                height: f.height,
            })
            .unwrap_or_default();
        // Drop nodes that carry no useful affordance.
        if r.role.is_none() && r.label.is_none() {
            continue;
        }
        if frame.is_empty() && r.label.is_none() {
            continue;
        }
        n += 1;
        elements.push(Element {
            r#ref: format!("e{n}"),
            role: r.role,
            label: r.label,
            value: value_to_string(r.value),
            frame,
            enabled: r.enabled.unwrap_or(true),
            actions: r.custom_actions,
        });
    }
    Ok(Snapshot { elements })
}

/// Persist a snapshot's ref→element map (at `path`) for later locator resolution.
pub fn save_cache(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(&snap.elements)?)?;
    Ok(())
}

/// Load the last cached snapshot (from `path`) for `@ref` resolution.
pub fn load_cache(path: &Path) -> Result<Snapshot> {
    let data = std::fs::read_to_string(path)
        .map_err(|_| agent_controller_core::anyhow!("no snapshot cache; run `snapshot` first"))?;
    let elements: Vec<Element> = serde_json::from_str(&data)?;
    Ok(Snapshot { elements })
}
