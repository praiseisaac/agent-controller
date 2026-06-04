//! Parse `uiautomator dump` XML into a core [`Snapshot`] with `@ref`s, and a
//! tiny on-disk ref cache so `click @eN` works across CLI invocations.

use agent_controller_core::{anyhow, Element, Rect, Result, Snapshot};
use std::path::Path;

fn parse_bounds(b: &str) -> Option<Rect> {
    // "[x1,y1][x2,y2]"
    let inner = b.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (a, c) = inner.split_once("][")?;
    let (x1, y1) = a.split_once(',')?;
    let (x2, y2) = c.split_once(',')?;
    let x1: f64 = x1.trim().parse().ok()?;
    let y1: f64 = y1.trim().parse().ok()?;
    let x2: f64 = x2.trim().parse().ok()?;
    let y2: f64 = y2.trim().parse().ok()?;
    Some(Rect {
        x: x1,
        y: y1,
        width: x2 - x1,
        height: y2 - y1,
    })
}

fn short_class(c: &str) -> String {
    c.rsplit('.').next().unwrap_or(c).to_string()
}

pub fn parse(xml: &str) -> Result<Snapshot> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| anyhow!("uiautomator XML: {e}"))?;
    let mut elements = Vec::new();
    let mut n = 0usize;
    for node in doc.descendants().filter(|d| d.has_tag_name("node")) {
        let frame = node
            .attribute("bounds")
            .and_then(parse_bounds)
            .unwrap_or_default();
        if frame.is_empty() {
            continue;
        }
        let text = node.attribute("text").unwrap_or("").trim().to_string();
        let desc = node.attribute("content-desc").unwrap_or("").trim().to_string();
        let rid = node.attribute("resource-id").unwrap_or("");
        let class = node.attribute("class").unwrap_or("");
        let clickable = node.attribute("clickable") == Some("true");
        let enabled = node.attribute("enabled") != Some("false");
        let label = if !text.is_empty() {
            Some(text.clone())
        } else if !desc.is_empty() {
            Some(desc.clone())
        } else {
            None
        };
        // Skip pure layout nodes with no affordance.
        if !clickable && label.is_none() && rid.is_empty() {
            continue;
        }
        n += 1;
        let mut actions = Vec::new();
        if clickable {
            actions.push("click".to_string());
        }
        elements.push(Element {
            r#ref: format!("e{n}"),
            role: Some(if class.is_empty() {
                "node".into()
            } else {
                short_class(class)
            }),
            label,
            value: (!text.is_empty()).then_some(text),
            frame,
            enabled,
            actions,
        });
    }
    Ok(Snapshot { elements })
}

pub fn save_cache(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(&snap.elements)?)?;
    Ok(())
}

pub fn load_cache(path: &Path) -> Result<Snapshot> {
    let data =
        std::fs::read_to_string(path).map_err(|_| anyhow!("no snapshot cache; run `snapshot` first"))?;
    let elements: Vec<Element> = serde_json::from_str(&data)?;
    Ok(Snapshot { elements })
}
