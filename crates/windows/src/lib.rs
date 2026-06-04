#![cfg(target_os = "windows")]
//! Windows desktop backend via UI Automation (`IUIAutomation`) + `SendInput`.
//! Windows-only; compiles to an empty crate elsewhere.
//!
//! First cut — build on Windows and iterate against the compiler; the
//! `windows` crate's COM/UIA surface is version-sensitive.
//!
//! COM objects are apartment-threaded (not Send/Sync), and `Controller` is
//! `Send + Sync`, so the controller stores only plain data (target window
//! handle); each method initializes COM and builds the automation object on its
//! own thread. Fine for a one-shot CLI.

use agent_controller_core::{
    anyhow, Backend, BackendFactory, Capabilities, Controller, Element, Identity, Image, Locator,
    Options, Rect, Result, ScrollDir, SessionPaths, SessionRecord, SessionStore, Snapshot,
};
use async_trait::async_trait;
use std::path::Path;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, SRCCOPY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    UIA_InvokePatternId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY, VK_BACK,
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, SetForegroundWindow, SM_CXSCREEN,
    SM_CYSCREEN,
};

fn com_init() -> Result<()> {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // S_OK / S_FALSE (already initialized) are both fine.
        if hr.is_err() {
            return Err(anyhow!("CoInitializeEx failed: {hr:?}"));
        }
    }
    Ok(())
}

fn automation() -> Result<IUIAutomation> {
    com_init()?;
    unsafe {
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| anyhow!("creating UIAutomation: {e}"))
    }
}

/// The UIA element for a window handle (or the desktop root if `hwnd` is 0).
fn element_for(auto: &IUIAutomation, hwnd: isize) -> Result<IUIAutomationElement> {
    unsafe {
        if hwnd == 0 {
            auto.GetRootElement().map_err(|e| anyhow!("GetRootElement: {e}"))
        } else {
            auto.ElementFromHandle(HWND(hwnd as *mut _))
                .map_err(|e| anyhow!("ElementFromHandle: {e}"))
        }
    }
}

fn control_type_name(id: i32) -> &'static str {
    // UIA_*ControlTypeId values (Win32_UI_Accessibility), common subset.
    match id {
        50000 => "Button",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50008 => "ListItem",
        50011 => "MenuItem",
        50020 => "Text",
        50021 => "Tree",
        50024 => "Window",
        50026 => "Group",
        50029 => "Tab",
        50030 => "TabItem",
        50031 => "Document",
        50032 => "Pane",
        50002 => "CheckBox",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50003 => "ComboBox",
        _ => "Control",
    }
}

fn rect_of(el: &IUIAutomationElement) -> Rect {
    unsafe {
        match el.CurrentBoundingRectangle() {
            Ok(r) => Rect {
                x: r.left as f64,
                y: r.top as f64,
                width: (r.right - r.left) as f64,
                height: (r.bottom - r.top) as f64,
            },
            Err(_) => Rect::default(),
        }
    }
}

fn name_of(el: &IUIAutomationElement) -> Option<String> {
    unsafe {
        el.CurrentName().ok().map(|b| b.to_string()).filter(|s| !s.is_empty())
    }
}

/// Walk the control view from `root`, collecting interactive/labeled elements.
fn walk(auto: &IUIAutomation, root: &IUIAutomationElement) -> Result<Vec<Element>> {
    let mut out = Vec::new();
    let mut n = 0usize;
    unsafe {
        let walker = auto.ControlViewWalker().map_err(|e| anyhow!("ControlViewWalker: {e}"))?;
        let mut stack = vec![(root.clone(), 0usize)];
        let mut budget = 4000usize;
        while let Some((el, depth)) = stack.pop() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let frame = rect_of(&el);
            let label = name_of(&el);
            let ctype = el.CurrentControlType().map(|c| c.0).unwrap_or(0);
            let role = control_type_name(ctype);
            if !frame.is_empty() && (label.is_some() || role == "Button" || role == "Edit") {
                n += 1;
                let mut actions = Vec::new();
                if el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).is_ok() {
                    actions.push("Invoke".to_string());
                }
                out.push(Element {
                    r#ref: format!("e{n}"),
                    role: Some(role.to_string()),
                    label,
                    value: None,
                    frame,
                    enabled: true,
                    actions,
                });
            }
            if depth < 40 {
                // Push children (control view).
                if let Ok(mut child) = walker.GetFirstChildElement(&el) {
                    loop {
                        stack.push((child.clone(), depth + 1));
                        match walker.GetNextSiblingElement(&child) {
                            Ok(next) => child = next,
                            Err(_) => break,
                        }
                    }
                }
            }
        }
    }
    Ok(out)
}

fn find_by_label(auto: &IUIAutomation, root: &IUIAutomationElement, needle: &str) -> Option<IUIAutomationElement> {
    let needle = needle.to_lowercase();
    unsafe {
        let walker = auto.ControlViewWalker().ok()?;
        let mut stack = vec![root.clone()];
        let mut budget = 4000usize;
        while let Some(el) = stack.pop() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Some(name) = name_of(&el) {
                if name.to_lowercase().contains(&needle) {
                    return Some(el);
                }
            }
            if let Ok(mut child) = walker.GetFirstChildElement(&el) {
                loop {
                    stack.push(child.clone());
                    match walker.GetNextSiblingElement(&child) {
                        Ok(next) => child = next,
                        Err(_) => break,
                    }
                }
            }
        }
    }
    None
}

fn invoke(el: &IUIAutomationElement) -> bool {
    unsafe {
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            return p.Invoke().is_ok();
        }
    }
    false
}

// ---- input (SendInput) ----------------------------------------------------

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse_click(x: i64, y: i64) {
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let ax = (x as f64 / sw.max(1) as f64 * 65535.0) as i32;
    let ay = (y as f64 / sh.max(1) as f64 * 65535.0) as i32;
    let mk = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: ax,
                dy: ay,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE | flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&[mk(MOUSEEVENTF_LEFTDOWN), mk(MOUSEEVENTF_LEFTUP)]);
}

fn mouse_wheel(delta: i32) {
    let i = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&[i]);
}

fn key_unit(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS, up: bool) -> INPUT {
    let mut f = flags;
    if up {
        f |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: f,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn type_unicode(text: &str) {
    let mut inputs = Vec::new();
    for u in text.encode_utf16() {
        inputs.push(key_unit(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE, false));
        inputs.push(key_unit(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE, true));
    }
    if !inputs.is_empty() {
        send(&inputs);
    }
}

fn named_vk(name: &str) -> Option<VIRTUAL_KEY> {
    Some(match name {
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "space" => VK_SPACE,
        "backspace" | "delete" => VK_BACK,
        "escape" | "esc" => VK_ESCAPE,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        s if s.len() == 1 => {
            let c = s.chars().next().unwrap().to_ascii_uppercase();
            VIRTUAL_KEY(c as u16)
        }
        _ => return None,
    })
}

// ---- screenshot (GDI BitBlt) ----------------------------------------------

fn capture(hwnd: isize) -> Result<Vec<u8>> {
    unsafe {
        let (x, y, w, h) = if hwnd != 0 {
            let mut r = RECT::default();
            GetWindowRect(HWND(hwnd as *mut _), &mut r).ok();
            (r.left, r.top, r.right - r.left, r.bottom - r.top)
        } else {
            (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        };
        if w <= 0 || h <= 0 {
            return Err(anyhow!("window has zero size"));
        }
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmp: HBITMAP = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY).ok();

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        // BGRA → RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let img = image::RgbaImage::from_raw(w as u32, h as u32, buf)
            .ok_or_else(|| anyhow!("bad image buffer"))?;
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png)
            .map_err(|e| anyhow!("png encode: {e}"))?;
        Ok(png.into_inner())
    }
}

// ---- controller -----------------------------------------------------------

pub struct WindowsController {
    target: String,
    /// Foreground/target window handle as an integer (Send+Sync); 0 = desktop.
    hwnd: isize,
    paths: SessionPaths,
}

fn save_cache(path: &Path, snap: &Snapshot) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, serde_json::to_string(&snap.elements)?)?;
    Ok(())
}
fn load_cache(path: &Path) -> Result<Snapshot> {
    let data = std::fs::read_to_string(path)
        .map_err(|_| anyhow!("no snapshot cache; run `snapshot` first"))?;
    Ok(Snapshot {
        elements: serde_json::from_str(&data)?,
    })
}

#[async_trait]
impl Controller for WindowsController {
    async fn navigate(&self, target: &str) -> Result<()> {
        // Launch/open via the shell, then try to foreground.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .map_err(|e| anyhow!("launching {target:?}: {e}"))?;
        if self.hwnd != 0 {
            unsafe {
                let _ = SetForegroundWindow(HWND(self.hwnd as *mut _));
            }
        }
        Ok(())
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let auto = automation()?;
        let root = element_for(&auto, self.hwnd)?;
        let snap = Snapshot {
            elements: walk(&auto, &root)?,
        };
        save_cache(&self.paths.refs, &snap)?;
        Ok(snap)
    }

    async fn click(&self, loc: &Locator) -> Result<()> {
        match loc {
            Locator::Point { x, y } => {
                mouse_click(*x as i64, *y as i64);
                Ok(())
            }
            Locator::Css(_) => Err(anyhow!("CSS selectors are not supported by the windows backend")),
            Locator::Ref(r) => {
                let snap = load_cache(&self.paths.refs)?;
                let el = snap
                    .elements
                    .iter()
                    .find(|e| &e.r#ref == r)
                    .ok_or_else(|| anyhow!("no cached element @{r}; run `snapshot` first"))?;
                let (cx, cy) = el.frame.center();
                mouse_click(cx as i64, cy as i64);
                Ok(())
            }
            Locator::Label(s) | Locator::Text(s) => self.click_label(s),
            Locator::Role { role, name } => {
                // Best-effort: match on the name if given, else first of that role.
                self.click_label(name.as_deref().unwrap_or(role))
            }
        }
    }

    async fn type_text(&self, text: &str) -> Result<()> {
        type_unicode(text);
        Ok(())
    }

    async fn press(&self, key: &str) -> Result<()> {
        let mut mods: Vec<VIRTUAL_KEY> = Vec::new();
        let mut base = None;
        for part in key.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.push(VIRTUAL_KEY(0x11)),  // VK_CONTROL
                "shift" => mods.push(VIRTUAL_KEY(0x10)),             // VK_SHIFT
                "alt" => mods.push(VIRTUAL_KEY(0x12)),               // VK_MENU
                "cmd" | "win" | "super" | "meta" => mods.push(VIRTUAL_KEY(0x5B)), // VK_LWIN
                other => base = Some(other.to_string()),
            }
        }
        let base = base.ok_or_else(|| anyhow!("no base key in {key:?}"))?;
        let vk = named_vk(&base).ok_or_else(|| anyhow!("unknown key: {base}"))?;
        let mut inputs = Vec::new();
        for m in &mods {
            inputs.push(key_unit(*m, 0, KEYBD_EVENT_FLAGS(0), false));
        }
        inputs.push(key_unit(vk, 0, KEYBD_EVENT_FLAGS(0), false));
        inputs.push(key_unit(vk, 0, KEYBD_EVENT_FLAGS(0), true));
        for m in mods.iter().rev() {
            inputs.push(key_unit(*m, 0, KEYBD_EVENT_FLAGS(0), true));
        }
        send(&inputs);
        Ok(())
    }

    async fn scroll(&self, dir: ScrollDir, amount: i32) -> Result<()> {
        let ticks = (amount.max(1) / 100).max(1);
        let unit = 120; // WHEEL_DELTA
        let delta = match dir {
            ScrollDir::Up => unit,
            ScrollDir::Down => -unit,
            // Horizontal wheel omitted in this first cut.
            ScrollDir::Left | ScrollDir::Right => return Err(anyhow!("horizontal scroll not yet supported on windows")),
        };
        for _ in 0..ticks {
            mouse_wheel(delta);
        }
        Ok(())
    }

    async fn screenshot(&self) -> Result<Image> {
        let data = capture(self.hwnd)?;
        Ok(Image {
            data,
            format: "png".into(),
        })
    }

    fn backend(&self) -> Backend {
        Backend::Windows
    }

    fn target(&self) -> String {
        self.target.clone()
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

impl WindowsController {
    fn click_label(&self, label: &str) -> Result<()> {
        let auto = automation()?;
        let root = element_for(&auto, self.hwnd)?;
        let el = find_by_label(&auto, &root, label)
            .ok_or_else(|| anyhow!("no element matching {label:?}"))?;
        if invoke(&el) {
            return Ok(());
        }
        let (cx, cy) = rect_of(&el).center();
        mouse_click(cx as i64, cy as i64);
        Ok(())
    }
}

/// Session-aware entry point for the Windows backend.
pub struct WindowsFactory;

#[async_trait]
impl BackendFactory for WindowsFactory {
    fn backend(&self) -> Backend {
        Backend::Windows
    }

    async fn identify(&self, opts: &Options) -> Result<Identity> {
        let name = opts
            .app
            .clone()
            .or_else(|| opts.session.clone())
            .unwrap_or_else(|| "foreground".to_string());
        Ok(Identity {
            id: format!("windows/{}", name.replace(['/', ' ', '\\'], "-")),
            target: name,
        })
    }

    async fn open(
        &self,
        rec: &mut SessionRecord,
        store: &SessionStore,
        _opts: &Options,
    ) -> Result<Box<dyn Controller>> {
        // Bind to the current foreground window (a future improvement: resolve a
        // window by title/app for rec.target).
        let hwnd = unsafe { GetForegroundWindow().0 as isize };
        rec.runtime.alive = true;
        rec.config = serde_json::json!({ "hwnd": hwnd });
        let paths = store.paths(&rec.id)?;
        Ok(Box::new(WindowsController {
            target: rec.target.clone(),
            hwnd,
            paths,
        }))
    }
}
