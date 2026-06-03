//! Display geometry. All clicks/AX frames/screenshots use the global Quartz
//! coordinate space (points, top-left origin of the main display, spanning all
//! monitors). This module exposes each display's bounds and backing scale so
//! pixel-derived coordinates (from screenshots) can be mapped to points — the
//! one place multi-screen / mixed-DPI matters.

use core_graphics::display::CGDisplay;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DisplayInfo {
    pub id: u32,
    /// Global-space bounds in points.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Pixels per point (Retina = 2.0); can differ per display in mixed-DPI setups.
    pub scale: f64,
    pub main: bool,
}

impl DisplayInfo {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// All active displays with bounds + backing scale.
pub fn displays() -> Vec<DisplayInfo> {
    let main_id = CGDisplay::main().id;
    CGDisplay::active_displays()
        .unwrap_or_default()
        .into_iter()
        .map(|id| {
            let d = CGDisplay::new(id);
            let b = d.bounds();
            // Backing scale from the display MODE: CGDisplayPixelsWide reports
            // points (not backing pixels) in scaled Retina modes, so derive scale
            // from pixel_width/width of the current mode instead.
            let scale = d
                .display_mode()
                .map(|m| {
                    let w = m.width() as f64;
                    if w > 0.0 {
                        m.pixel_width() as f64 / w
                    } else {
                        1.0
                    }
                })
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0);
            DisplayInfo {
                id,
                x: b.origin.x,
                y: b.origin.y,
                width: b.size.width,
                height: b.size.height,
                scale,
                main: id == main_id,
            }
        })
        .collect()
}

/// Backing scale of the display containing the given global point (1.0 if none).
pub fn scale_at(x: f64, y: f64) -> f64 {
    displays()
        .into_iter()
        .find(|d| d.contains(x, y))
        .map(|d| d.scale)
        .unwrap_or(1.0)
}
