//! Permission inspection + guided prompts for the macOS backend.

use accessibility_sys::{
    kAXTrustedCheckOptionPrompt, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
};
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_graphics::access::ScreenCaptureAccess;

/// Is Accessibility granted to the controlling process?
pub fn ax_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Check Accessibility and, if missing, trigger the system prompt + add the
/// controlling app to the Accessibility list pane.
pub fn ax_request() -> bool {
    unsafe {
        let key = kAXTrustedCheckOptionPrompt;
        let value = core_foundation::boolean::CFBoolean::true_value();
        let opts = CFDictionary::from_CFType_pairs(&[(
            core_foundation::string::CFString::wrap_under_get_rule(key),
            value.as_CFType(),
        )]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef())
    }
}

/// Is Screen Recording granted?
pub fn screen_recording_ok() -> bool {
    ScreenCaptureAccess.preflight()
}

/// Trigger the Screen Recording prompt / settings pane.
pub fn screen_recording_request() -> bool {
    ScreenCaptureAccess.request()
}
