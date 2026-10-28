//! Realistic input: drive the page the way a person would, for demos and for
//! recordings a reviewer watches. Off by default; `AGENT_CONTROLLER_REALISTIC=1`
//! turns it on. Off, every verb is the instant input it always was.
//!
//! - **Moves** follow a slightly curved cubic Bézier, ease in and out, wobble a
//!   little, and a long move overshoots by a few px and settles back.
//! - **Clicks** wait a short, varied beat before pressing.
//! - **Typing** comes one key at a time in uneven bursts, with a longer gap now
//!   and then and after word and symbol boundaries.
//! - **A drawn cursor** (13×17 px, an orange ring on click) is put into every
//!   page so a screencast shows the pointer. It is `aria-hidden`,
//!   `pointer-events:none` and carries no role, so snapshots never see it, and
//!   it hides when a mouse move the driver did not make comes in (the person's
//!   own pointer). Nothing here brings the window to the front.
//!
//! The path and timing generator is pure and seeded (`Rng`), so tests pin it;
//! `AGENT_CONTROLLER_REALISTIC_SEED` pins a live run the same way.

use std::time::{SystemTime, UNIX_EPOCH};

/// Env var: `1`/`true`/`on` drives the page like a person (chrome).
pub const ENV_REALISTIC: &str = "AGENT_CONTROLLER_REALISTIC";
/// Env var: a fixed seed for the realistic timings (tests, reproducible demos).
pub const ENV_REALISTIC_SEED: &str = "AGENT_CONTROLLER_REALISTIC_SEED";

/// Whether an `AGENT_CONTROLLER_REALISTIC` value turns realistic input on.
/// Unset, empty, or any no → off.
pub fn enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// The seed a run uses: the env's when it parses, else the clock's.
pub fn seed(value: Option<&str>) -> u64 {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15)
        })
}

/// splitmix64: small, fast, and the same sequence for the same seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + unit * (hi - lo)
    }
}

/// One pointer step: where to move, then how long to wait before the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveStep {
    pub x: f64,
    pub y: f64,
    pub wait_ms: u64,
}

/// Moves longer than this overshoot the target and settle back.
pub const OVERSHOOT_OVER_PX: f64 = 120.0;

fn bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
}

/// Ease-in-out (cubic): slow off the mark, fast through the middle, slow in.
fn ease(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// One curved leg from `from` to `to` over about `duration_ms`. The last step
/// lands exactly on `to`; the ones before wobble by under a pixel.
fn leg(rng: &mut Rng, from: (f64, f64), to: (f64, f64), duration_ms: f64, out: &mut Vec<MoveStep>) {
    let (x0, y0) = from;
    let (x, y) = to;
    let (dx, dy) = (x - x0, y - y0);
    let dist = dx.hypot(dy).max(1.0);
    let (nx, ny) = (-dy / dist, dx / dist);
    let bow = rng.range(-0.25, 0.25) * dist.min(400.0);
    let (a, b) = (rng.range(0.2, 0.4), rng.range(0.6, 0.85));
    let (c1x, c1y) = (x0 + dx * a + nx * bow, y0 + dy * a + ny * bow);
    let (c2x, c2y) = (x0 + dx * b + nx * bow * 0.5, y0 + dy * b + ny * bow * 0.5);
    let steps = ((duration_ms / 16.0).round() as usize).max(10);
    for i in 1..=steps {
        let t = ease(i as f64 / steps as f64);
        let jitter = if i < steps { rng.range(-0.6, 0.6) } else { 0.0 };
        out.push(MoveStep {
            x: bezier(x0, c1x, c2x, x, t) + jitter,
            y: bezier(y0, c1y, c2y, y, t) + jitter,
            wait_ms: rng.range(12.0, 20.0) as u64,
        });
    }
}

/// The pointer's whole glide from `from` to `to`, ending exactly on `to`.
pub fn mouse_path(rng: &mut Rng, from: (f64, f64), to: (f64, f64)) -> Vec<MoveStep> {
    let dist = (to.0 - from.0).hypot(to.1 - from.1);
    let duration = (260.0 + dist * rng.range(0.9, 1.3)).min(1100.0);
    let mut out = Vec::new();
    if dist > OVERSHOOT_OVER_PX {
        let (ux, uy) = ((to.0 - from.0) / dist, (to.1 - from.1) / dist);
        let past = rng.range(4.0, 10.0);
        let over = (to.0 + ux * past, to.1 + uy * past);
        leg(rng, from, over, duration * 0.88, &mut out);
        if let Some(last) = out.last_mut() {
            last.wait_ms += rng.range(30.0, 70.0) as u64;
        }
        let settle = rng.range(90.0, 150.0);
        leg(rng, over, to, settle, &mut out);
    } else {
        leg(rng, from, to, duration, &mut out);
    }
    out
}

/// Where in an element's box to click: near its centre, never on its edge.
pub fn click_point(rng: &mut Rng, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
    (
        x + w / 2.0 + rng.range(-0.2, 0.2) * w,
        y + h / 2.0 + rng.range(-0.2, 0.2) * h,
    )
}

/// The beat before a click presses (ms).
pub fn click_pause_ms(rng: &mut Rng) -> u64 {
    rng.range(120.0, 280.0) as u64
}

/// How long a pressed button or key is held (ms).
pub fn hold_ms(rng: &mut Rng) -> u64 {
    rng.range(25.0, 90.0) as u64
}

/// One typed character: how long its key is held, then the gap to the next.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyStroke {
    pub ch: char,
    pub hold_ms: u64,
    pub gap_ms: u64,
}

/// The rhythm for typing `text`: runs of quick keys, a hesitation after a
/// space or symbol, and a longer gap at the end of each burst — never smooth.
pub fn typing(rng: &mut Rng, text: &str) -> Vec<KeyStroke> {
    let mut burst = rng.range(3.0, 7.0).round() as i32;
    text.chars()
        .map(|ch| {
            let hold = rng.range(25.0, 70.0) as u64;
            let mut gap = rng.range(45.0, 120.0);
            if matches!(ch, ' ' | '@' | '.' | '+' | '-' | '_' | '/' | ',') {
                gap += rng.range(120.0, 320.0);
            }
            burst -= 1;
            if burst <= 0 {
                gap += rng.range(150.0, 420.0);
                burst = rng.range(3.0, 8.0).round() as i32;
            }
            KeyStroke {
                ch,
                hold_ms: hold,
                gap_ms: gap as u64,
            }
        })
        .collect()
}

/// The isolated world the drawn cursor lives in. Its script and its state are
/// never in the page's own world: the page sees two `aria-hidden` elements and
/// nothing else — no global, no function it could call or replace.
pub const CURSOR_WORLD: &str = "agent-controller-cursor";

/// The drawn cursor, put into every document, run in [`CURSOR_WORLD`]. Each
/// CLI call is a new CDP session and so a new isolated world, so its state is
/// kept on the cursor element, not in a world's globals: `data-driving="1"`
/// while the driver moves (any other move is the person's pointer, and the
/// drawn one hides until the driver moves again), and the world that mounted
/// it keeps its listeners for the life of the document; a later world finds
/// the element and adds none.
pub const CURSOR_JS: &str = r#"(() => {
  const SEL = '[data-agent-controller-cursor="pointer"]';
  const q = () => document.querySelector(SEL);
  const mount = () => {
    if (q() || !document.documentElement) return;
    const quiet = (el, part) => { el.setAttribute('aria-hidden', 'true'); el.setAttribute('data-agent-controller-cursor', part); el.inert = true; };
    const c = document.createElement('div');
    c.innerHTML = '<svg width="13" height="17" viewBox="0 0 22 28"><path d="M1 1 L1 22 L6.5 16.8 L10.2 26 L13.6 24.6 L9.9 15.5 L17 15.5 Z" fill="black" stroke="white" stroke-width="1.6" stroke-linejoin="round"/></svg>';
    Object.assign(c.style, { position: 'fixed', left: '0px', top: '0px', width: '13px', height: '17px',
      pointerEvents: 'none', zIndex: '2147483647', opacity: '0', transform: 'translate(-40px,-40px)' });
    const ring = document.createElement('div');
    Object.assign(ring.style, { position: 'fixed', width: '22px', height: '22px', marginLeft: '-11px', marginTop: '-11px',
      borderRadius: '50%', border: '2px solid rgba(255,140,0,0.85)', pointerEvents: 'none', zIndex: '2147483646',
      opacity: '0', left: '0px', top: '0px' });
    quiet(c, 'pointer'); quiet(ring, 'ring');
    document.documentElement.appendChild(ring);
    document.documentElement.appendChild(c);
    addEventListener('mousemove', e => {
      const p = q(); if (!p) return;
      if (p.dataset.driving !== '1') { p.style.opacity = '0'; return; }
      p.style.opacity = '1'; p.style.transform = 'translate(' + e.clientX + 'px,' + e.clientY + 'px)';
    }, true);
    addEventListener('mousedown', e => {
      const p = q(); if (!p || p.dataset.driving !== '1') return;
      ring.style.left = e.clientX + 'px'; ring.style.top = e.clientY + 'px';
      ring.style.transition = 'none'; ring.style.opacity = '1'; ring.style.transform = 'scale(0.6)';
      requestAnimationFrame(() => { ring.style.transition = 'opacity 0.6s, transform 0.6s'; ring.style.opacity = '0'; ring.style.transform = 'scale(1.4)'; });
    }, true);
  };
  if (document.documentElement) mount(); else addEventListener('DOMContentLoaded', mount);
})()"#;

/// JS (for [`CURSOR_WORLD`]) marking the driver's own moves on or off.
pub fn driving_js(on: bool) -> String {
    format!(
        "(() => {{ const p = document.querySelector('[data-agent-controller-cursor=\"pointer\"]'); if (p) p.dataset.driving = '{}'; }})()",
        if on { "1" } else { "0" }
    )
}

/// JS (for [`CURSOR_WORLD`]) placing the drawn cursor at `(x, y)` without a
/// move: a fresh page, a resumed session.
pub fn place_js(x: f64, y: f64) -> String {
    format!(
        "(() => {{ const p = document.querySelector('[data-agent-controller-cursor=\"pointer\"]'); if (p) {{ p.style.opacity = '1'; p.style.transform = 'translate({x}px,{y}px)'; }} }})()"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_reads_yes_values_only() {
        for v in ["1", "true", "on", "YES", " on "] {
            assert!(enabled(Some(v)), "{v}");
        }
        for v in ["", "0", "off", "false", "no", "maybe"] {
            assert!(!enabled(Some(v)), "{v}");
        }
        assert!(!enabled(None), "unset is off: today's instant input");
    }

    #[test]
    fn seed_parses_or_falls_back() {
        assert_eq!(seed(Some("42")), 42);
        // an unparseable seed still gives one (the clock's), never a panic
        let _ = seed(Some("nope"));
    }

    #[test]
    fn same_seed_same_path() {
        let a = mouse_path(&mut Rng::new(7), (10.0, 10.0), (600.0, 400.0));
        let b = mouse_path(&mut Rng::new(7), (10.0, 10.0), (600.0, 400.0));
        assert_eq!(a, b);
        let c = mouse_path(&mut Rng::new(8), (10.0, 10.0), (600.0, 400.0));
        assert_ne!(a, c);
    }

    #[test]
    fn path_ends_exactly_on_target() {
        for seed in 0..50 {
            for to in [(600.0, 400.0), (30.0, 25.0), (10.0, 10.0)] {
                let p = mouse_path(&mut Rng::new(seed), (10.0, 10.0), to);
                let last = p.last().unwrap();
                assert_eq!((last.x, last.y), to, "seed {seed}");
                assert!(p.len() >= 10);
            }
        }
    }

    #[test]
    fn long_move_overshoots_then_settles() {
        for seed in 0..50 {
            let p = mouse_path(&mut Rng::new(seed), (0.0, 100.0), (500.0, 100.0));
            let max_x = p.iter().map(|s| s.x).fold(f64::MIN, f64::max);
            // past the target by a few px (4-10, plus under a px of wobble), then back onto it
            assert!(max_x > 503.0 && max_x < 511.0, "seed {seed}: max x {max_x}");
        }
    }

    #[test]
    fn short_move_never_overshoots() {
        for seed in 0..50 {
            let p = mouse_path(&mut Rng::new(seed), (0.0, 0.0), (80.0, 0.0));
            let max_x = p.iter().map(|s| s.x).fold(f64::MIN, f64::max);
            assert!(max_x <= 80.6, "seed {seed}: max x {max_x}");
        }
    }

    #[test]
    fn path_curves_and_eases() {
        let p = mouse_path(&mut Rng::new(3), (0.0, 0.0), (400.0, 0.0));
        // curved: somewhere along the way it leaves the straight line by more than the wobble
        assert!(
            p.iter().any(|s| s.y.abs() > 2.0),
            "a straight line, not a curve"
        );
        // eased: the first and last steps of the first leg are shorter than the middle ones
        let d: Vec<f64> = std::iter::once((0.0, 0.0))
            .chain(p.iter().map(|s| (s.x, s.y)))
            .collect::<Vec<_>>()
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
            .collect();
        let mid = d[d.len() / 3];
        assert!(d[0] < mid, "starts slow: {} vs {mid}", d[0]);
        for s in &p {
            assert!((12..=90).contains(&s.wait_ms), "wait {}", s.wait_ms);
        }
    }

    #[test]
    fn click_point_stays_inside_the_box() {
        let mut rng = Rng::new(1);
        for _ in 0..200 {
            let (x, y) = click_point(&mut rng, 100.0, 50.0, 40.0, 20.0);
            assert!(x > 100.0 && x < 140.0 && y > 50.0 && y < 70.0, "{x},{y}");
        }
        let mut rng = Rng::new(2);
        for _ in 0..200 {
            assert!((120..280).contains(&click_pause_ms(&mut rng)));
        }
    }

    #[test]
    fn typing_is_uneven_with_longer_gaps() {
        let text = "person@example.com types like a person";
        let a = typing(&mut Rng::new(11), text);
        assert_eq!(a, typing(&mut Rng::new(11), text));
        assert_eq!(a.iter().map(|k| k.ch).collect::<String>(), text);
        let gaps: Vec<u64> = a.iter().map(|k| k.gap_ms).collect();
        let (min, max) = (*gaps.iter().min().unwrap(), *gaps.iter().max().unwrap());
        assert!(min >= 45 && max <= 120 + 320 + 420, "{min}..{max}");
        // not smooth: some gaps are well past the quick ones
        assert!(gaps.iter().filter(|&&g| g > 200).count() >= 3, "{gaps:?}");
        assert!(gaps.iter().filter(|&&g| g < 120).count() >= 5, "{gaps:?}");
        // the gap after a space or @ is always a hesitation
        for k in a.iter().filter(|k| k.ch == ' ' || k.ch == '@') {
            assert!(k.gap_ms >= 165, "{k:?}");
        }
    }

    #[test]
    fn cursor_is_hidden_from_snapshots() {
        assert!(CURSOR_JS.contains("aria-hidden"));
        assert!(CURSOR_JS.contains("pointerEvents: 'none'"));
        // a role would put it in the snapshot's selector list
        assert!(!CURSOR_JS.contains("role"));
    }

    #[test]
    fn cursor_leaves_no_global_in_any_world() {
        // its state is on the element (RI1): nothing on window for the page to read, call or replace
        for js in [CURSOR_JS.to_string(), driving_js(true), driving_js(false), place_js(1.0, 2.0)] {
            assert!(!js.contains("window."), "{js}");
            assert!(!js.contains("__ac"), "{js}");
        }
        assert!(driving_js(true).contains("driving = '1'"));
        assert!(driving_js(false).contains("driving = '0'"));
        assert!(place_js(12.5, 30.0).contains("translate(12.5px,30px)"));
    }
}
