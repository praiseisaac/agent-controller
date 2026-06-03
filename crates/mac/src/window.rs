//! CGWindowList lookup: find an app's frontmost normal window id, so we can
//! capture that specific window (even when occluded) instead of a screen region.

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::{CFNumber, CFNumberRef};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionOnScreenOnly, kCGWindowNumber, kCGWindowOwnerPID,
};
use std::ffi::c_void;

unsafe fn dict_i64(dict: &CFDictionary, key: *const c_void) -> Option<i64> {
    let value = dict.find(key)?;
    let num = CFNumber::wrap_under_get_rule(*value as CFNumberRef);
    num.to_i64()
}

/// The window id of `pid`'s frontmost on-screen normal window (layer 0).
pub fn frontmost_window_id(pid: i32) -> Option<u32> {
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let info = copy_window_info(option, kCGNullWindowID)?;
    // On-screen list is ordered front-to-back: the first match is frontmost.
    for i in 0..info.len() {
        let item = info.get(i)?;
        let dict = unsafe { CFDictionary::wrap_under_get_rule(*item as *const _) };
        let owner = unsafe { dict_i64(&dict, kCGWindowOwnerPID as *const c_void) };
        if owner != Some(pid as i64) {
            continue;
        }
        let layer = unsafe { dict_i64(&dict, kCGWindowLayer as *const c_void) };
        if layer != Some(0) {
            continue;
        }
        if let Some(num) = unsafe { dict_i64(&dict, kCGWindowNumber as *const c_void) } {
            return Some(num as u32);
        }
    }
    None
}
