//! Spike: validate the Accessibility FFI path before building the full backend.
//! Checks AX trust, resolves the frontmost app, and walks its AX tree.

use accessibility::{AXAttribute, AXUIElement};
use accessibility_sys::AXIsProcessTrusted;

fn frontmost_pid() -> i32 {
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to get unix id of first application process whose frontmost is true",
        ])
        .output()
        .expect("osascript");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

fn attr_string(e: &AXUIElement, a: &AXAttribute<core_foundation::string::CFString>) -> String {
    e.attribute(a).map(|s| s.to_string()).unwrap_or_default()
}

fn walk(e: &AXUIElement, depth: usize, budget: &mut usize) {
    if *budget == 0 || depth > 4 {
        return;
    }
    let role = attr_string(e, &AXAttribute::role());
    let title = attr_string(e, &AXAttribute::title());
    let role = if role.is_empty() { "?".into() } else { role };
    println!("{}{} {:?}", "  ".repeat(depth), role, title);
    *budget -= 1;
    if let Ok(children) = e.attribute(&AXAttribute::children()) {
        for child in children.iter() {
            walk(&child, depth + 1, budget);
            if *budget == 0 {
                break;
            }
        }
    }
}

fn main() {
    let trusted = unsafe { AXIsProcessTrusted() };
    println!("AX trusted: {trusted}");
    if !trusted {
        println!("Grant Accessibility to your terminal: System Settings ▸ Privacy & Security ▸ Accessibility");
    }
    let pid = frontmost_pid();
    println!("frontmost pid: {pid}");
    let app = AXUIElement::application(pid);
    println!("app role: {:?}", attr_string(&app, &AXAttribute::role()));
    println!("app title: {:?}", attr_string(&app, &AXAttribute::title()));
    let mut budget = 50usize;
    walk(&app, 0, &mut budget);
}
