use super::CMUX_BUNDLE_ID;
use core_foundation::{
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    geometry::CGRect,
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
    },
};
use objc2_app_kit::NSWorkspace;

#[derive(Clone, Copy)]
pub(super) struct WindowFrame {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
}

pub(super) fn cmux_is_frontmost() -> bool {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|application| application.bundleIdentifier())
        .is_some_and(|bundle| bundle.to_string() == CMUX_BUNDLE_ID)
}

pub(super) fn cmux_window_frame() -> Option<WindowFrame> {
    let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    if application.bundleIdentifier()?.to_string() != CMUX_BUNDLE_ID {
        return None;
    }
    let pid = i64::from(application.processIdentifier());
    let windows = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;
    let pid_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    windows.iter().find_map(|value| {
        let dictionary = unsafe {
            CFDictionary::<CFString, CFType>::wrap_under_get_rule(
                *value as core_foundation::dictionary::CFDictionaryRef,
            )
        };
        if number(&dictionary, &pid_key)? != pid || number(&dictionary, &layer_key)? != 0 {
            return None;
        }
        let bounds = dictionary.find(&bounds_key)?.downcast::<CFDictionary>()?;
        let bounds = CGRect::from_dict_representation(&bounds)?;
        (bounds.size.width >= 200.0 && bounds.size.height >= 100.0).then_some(WindowFrame {
            x: bounds.origin.x,
            y: bounds.origin.y,
            width: bounds.size.width,
        })
    })
}

fn number(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dictionary.find(key)?.downcast::<CFNumber>()?.to_i64()
}
