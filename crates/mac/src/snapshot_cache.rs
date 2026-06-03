//! Persist the last snapshot's `@ref` → element map so `click @eN` works across
//! separate CLI invocations.

use agent_controller_core::{anyhow, Element, Result, Snapshot};
use std::path::Path;

pub fn save(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(&snap.elements)?)?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Snapshot> {
    let data = std::fs::read_to_string(path)
        .map_err(|_| anyhow!("no snapshot cache; run `snapshot` first"))?;
    let elements: Vec<Element> = serde_json::from_str(&data)?;
    Ok(Snapshot { elements })
}
