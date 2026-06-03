//! Accessibility-tree reading: snapshot with `@ref`s, element lookup, frames,
//! and actions. The macOS analogue of a DOM snapshot.

use accessibility::{AXAttribute, AXUIElement};
use accessibility_sys::{kAXValueTypeCGPoint, kAXValueTypeCGSize, AXValueGetValue, AXValueRef};
use agent_controller_core::{anyhow, Element, Rect, Result, Snapshot};
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGSize};
use std::ffi::c_void;

/// Roles we always surface even without an action (useful to read/locate).
const INTERESTING: &[&str] = &[
    "AXButton",
    "AXTextField",
    "AXTextArea",
    "AXSecureTextField",
    "AXCheckBox",
    "AXRadioButton",
    "AXPopUpButton",
    "AXMenuButton",
    "AXMenuItem",
    "AXLink",
    "AXTab",
    "AXSlider",
    "AXComboBox",
    "AXStaticText",
    "AXCell",
    "AXRow",
    "AXImage",
    "AXDisclosureTriangle",
];

pub fn root_for(pid: i32) -> AXUIElement {
    AXUIElement::application(pid)
}

fn attr_string(e: &AXUIElement, a: &AXAttribute<CFString>) -> Option<String> {
    e.attribute(a)
        .ok()
        .map(|v| v.to_string())
        .filter(|s| !s.is_empty())
}

fn read_axvalue(e: &AXUIElement, name: &str, ty: u32, out: *mut c_void) -> bool {
    let attr = AXAttribute::<CFType>::new(&CFString::new(name));
    let Ok(v) = e.attribute(&attr) else {
        return false;
    };
    unsafe { AXValueGetValue(v.as_CFTypeRef() as AXValueRef, ty, out) }
}

/// On-screen frame in logical points (top-left origin).
pub fn frame_of(e: &AXUIElement) -> Rect {
    let mut r = Rect::default();
    let mut p = CGPoint::new(0.0, 0.0);
    if read_axvalue(e, "AXPosition", kAXValueTypeCGPoint, &mut p as *mut _ as *mut c_void) {
        r.x = p.x;
        r.y = p.y;
    }
    let mut s = CGSize::new(0.0, 0.0);
    if read_axvalue(e, "AXSize", kAXValueTypeCGSize, &mut s as *mut _ as *mut c_void) {
        r.width = s.width;
        r.height = s.height;
    }
    r
}

pub fn actions(e: &AXUIElement) -> Vec<String> {
    e.action_names()
        .ok()
        .map(|arr| arr.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn label(e: &AXUIElement) -> Option<String> {
    attr_string(e, &AXAttribute::title())
        .or_else(|| attr_string(e, &AXAttribute::description()))
}

fn value_str(e: &AXUIElement) -> Option<String> {
    let v = e.attribute(&AXAttribute::value()).ok()?;
    if v.instance_of::<CFString>() {
        let s = unsafe { CFString::wrap_under_get_rule(v.as_CFTypeRef() as CFStringRef) }.to_string();
        return if s.is_empty() { None } else { Some(s) };
    }
    None
}

fn include(role: &Option<String>, label: &Option<String>, acts: &[String], frame: &Rect) -> bool {
    if frame.is_empty() {
        return false;
    }
    if !acts.is_empty() {
        return true;
    }
    if let Some(r) = role.as_deref() {
        if INTERESTING.contains(&r) {
            return true;
        }
    }
    label.is_some()
}

/// Walk the tree and produce an `@ref`-tagged snapshot.
pub fn snapshot(root: &AXUIElement) -> Snapshot {
    let mut els = Vec::new();
    let mut n = 0usize;
    let mut budget = 4000usize;
    walk(root, &mut els, &mut n, 0, &mut budget);
    Snapshot { elements: els }
}

fn walk(e: &AXUIElement, els: &mut Vec<Element>, n: &mut usize, depth: usize, budget: &mut usize) {
    if depth > 60 || *budget == 0 {
        return;
    }
    *budget -= 1;
    let role = attr_string(e, &AXAttribute::role());
    let lbl = label(e);
    let acts = actions(e);
    let frame = frame_of(e);
    if include(&role, &lbl, &acts, &frame) {
        *n += 1;
        els.push(Element {
            r#ref: format!("e{n}"),
            role,
            label: lbl,
            value: value_str(e),
            frame,
            enabled: true,
            actions: acts,
        });
    }
    if let Ok(children) = e.attribute(&AXAttribute::children()) {
        for child in children.iter() {
            walk(&child, els, n, depth + 1, budget);
            if *budget == 0 {
                break;
            }
        }
    }
}

/// Find the first element whose (role, label) satisfies `pred`; returns it plus
/// its frame. Used for semantic locators (label/text/role).
pub fn find(
    root: &AXUIElement,
    pred: &dyn Fn(Option<&str>, Option<&str>) -> bool,
) -> Option<(AXUIElement, Rect)> {
    let mut budget = 4000usize;
    find_rec(root, pred, 0, &mut budget)
}

fn find_rec(
    e: &AXUIElement,
    pred: &dyn Fn(Option<&str>, Option<&str>) -> bool,
    depth: usize,
    budget: &mut usize,
) -> Option<(AXUIElement, Rect)> {
    if depth > 60 || *budget == 0 {
        return None;
    }
    *budget -= 1;
    let role = attr_string(e, &AXAttribute::role());
    let lbl = label(e);
    if pred(role.as_deref(), lbl.as_deref()) {
        return Some((e.clone(), frame_of(e)));
    }
    if let Ok(children) = e.attribute(&AXAttribute::children()) {
        for child in children.iter() {
            if let Some(found) = find_rec(&child, pred, depth + 1, budget) {
                return Some(found);
            }
            if *budget == 0 {
                break;
            }
        }
    }
    None
}

/// First window's frame, for screenshots.
pub fn first_window_frame(root: &AXUIElement) -> Option<Rect> {
    let windows = root.attribute(&AXAttribute::windows()).ok()?;
    let w = windows.iter().next()?;
    let f = frame_of(&w);
    if f.is_empty() {
        None
    } else {
        Some(f)
    }
}

/// Try to perform `AXPress` on `e`. Returns true on success.
pub fn press(e: &AXUIElement) -> bool {
    e.perform_action(&CFString::new("AXPress")).is_ok()
}

fn role_of(e: &AXUIElement) -> Option<String> {
    attr_string(e, &AXAttribute::role())
}

fn children_of(e: &AXUIElement) -> Vec<AXUIElement> {
    e.attribute(&AXAttribute::children())
        .map(|arr| arr.iter().map(|c| c.clone()).collect())
        .unwrap_or_default()
}

fn menu_bar(app: &AXUIElement) -> Option<AXUIElement> {
    let attr = AXAttribute::<CFType>::new(&CFString::new("AXMenuBar"));
    let v = app.attribute(&attr).ok()?;
    if !v.instance_of::<AXUIElement>() {
        return None;
    }
    Some(unsafe {
        AXUIElement::wrap_under_get_rule(v.as_CFTypeRef() as accessibility_sys::AXUIElementRef)
    })
}

/// Navigate the menu bar by title path (e.g. ["Format","Font","Bold"]) and
/// `AXPress` the leaf — routes the command to the app without opening menus
/// visually or bringing it forward.
pub fn press_menu_path(app: &AXUIElement, path: &[String]) -> Result<()> {
    let mut container = menu_bar(app).ok_or_else(|| anyhow!("app has no AXMenuBar"))?;
    for (i, part) in path.iter().enumerate() {
        let want = part.to_lowercase();
        let item = children_of(&container)
            .into_iter()
            .find(|c| attr_string(c, &AXAttribute::title()).map(|t| t.to_lowercase()) == Some(want.clone()))
            .ok_or_else(|| anyhow!("menu item {part:?} not found"))?;
        if i + 1 == path.len() {
            return item
                .perform_action(&CFString::new("AXPress"))
                .map_err(|e| anyhow!("AXPress {part:?} failed: {e:?}"));
        }
        container = children_of(&item)
            .into_iter()
            .find(|c| role_of(c).as_deref() == Some("AXMenu"))
            .ok_or_else(|| anyhow!("menu {part:?} has no submenu"))?;
    }
    Err(anyhow!("empty menu path"))
}

/// The app's currently focused UI element, if any.
pub fn focused(root: &AXUIElement) -> Option<AXUIElement> {
    let attr = AXAttribute::<CFType>::new(&CFString::new("AXFocusedUIElement"));
    let v = root.attribute(&attr).ok()?;
    if !v.instance_of::<AXUIElement>() {
        return None;
    }
    Some(unsafe {
        AXUIElement::wrap_under_get_rule(v.as_CFTypeRef() as accessibility_sys::AXUIElementRef)
    })
}

/// Read an element's text value (if it's a string).
pub fn value_of(e: &AXUIElement) -> Option<String> {
    value_str(e)
}

/// Set the focused text element's value to `text` (cursor-free, no activation).
/// Returns false if there's no focused element or it isn't settable.
pub fn set_focused_value(root: &AXUIElement, text: &str) -> bool {
    let Some(focused) = focused(root) else {
        return false;
    };
    focused
        .set_attribute(&AXAttribute::value(), CFString::new(text).as_CFType())
        .is_ok()
}

/// Select all text in the focused element via `AXSelectedTextRange` (cursor-free).
pub fn select_all_focused(root: &AXUIElement) -> bool {
    let Some(focused) = focused(root) else {
        return false;
    };
    let len = value_str(&focused).map(|s| s.encode_utf16().count()).unwrap_or(0);
    let range = core_foundation::base::CFRange {
        location: 0,
        length: len as isize,
    };
    let axvalue = unsafe {
        accessibility_sys::AXValueCreate(
            accessibility_sys::kAXValueTypeCFRange,
            &range as *const _ as *const std::ffi::c_void,
        )
    };
    if axvalue.is_null() {
        return false;
    }
    let cf = unsafe { CFType::wrap_under_create_rule(axvalue as core_foundation::base::CFTypeRef) };
    let attr = AXAttribute::<CFType>::new(&CFString::new("AXSelectedTextRange"));
    focused.set_attribute(&attr, cf).is_ok()
}
