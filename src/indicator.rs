use core_foundation::{
    base::{CFType, TCFType},
    date::CFDate,
    dictionary::CFDictionary,
    number::CFNumber,
    runloop::{
        CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, CFRunLoopTimerRef, kCFRunLoopCommonModes,
    },
    string::CFString,
};
use core_graphics::{
    geometry::CGRect,
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
    },
};
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained, rc::autoreleasepool};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor,
    NSFloatingWindowLevel, NSFont, NSPanel, NSTextAlignment, NSTextField,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    ffi::c_void,
    io,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

const CMUX_BUNDLE_ID: &str = "com.cmuxterm.app";
const HEIGHT: f64 = 24.0;
const RIGHT_INSET: f64 = 12.0;
const TOP_INSET: f64 = 3.0;
const POSITION_TICKS: u8 = 8;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Off,
    Ready,
    Listening,
    Transcribing,
    Working,
    Done,
    Error,
}

impl Phase {
    fn from_raw(value: u8) -> Self {
        match value {
            1 => Self::Ready,
            2 => Self::Listening,
            3 => Self::Transcribing,
            4 => Self::Working,
            5 => Self::Done,
            6 => Self::Error,
            _ => Self::Off,
        }
    }

    fn presentation(self) -> (&'static str, f64, Retained<NSColor>) {
        match self {
            Self::Off => ("○ Off", 48.0, NSColor::systemGrayColor()),
            Self::Ready => ("● Ready", 58.0, NSColor::systemGrayColor()),
            Self::Listening => ("● Listening", 76.0, NSColor::systemRedColor()),
            Self::Transcribing => ("● Transcribing", 94.0, NSColor::systemOrangeColor()),
            Self::Working => ("● Working", 70.0, NSColor::systemBlueColor()),
            Self::Done => ("✓ Done", 54.0, NSColor::systemGreenColor()),
            Self::Error => ("! Error", 50.0, NSColor::systemRedColor()),
        }
    }
}

#[derive(Clone, Default)]
pub struct Handle(Arc<AtomicU8>);

impl Handle {
    pub fn set(&self, phase: Phase) {
        self.0.store(phase as u8, Ordering::Release);
    }

    fn get(&self) -> Phase {
        Phase::from_raw(self.0.load(Ordering::Acquire))
    }
}

pub fn run(handle: Handle) -> io::Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("the CMUX indicator must run on the main thread"))?;
    let application = NSApplication::sharedApplication(mtm);
    application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut indicator = Box::new(Indicator::new(mtm, handle));
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: (&mut *indicator as *mut Indicator).cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    let timer = CFRunLoopTimer::new(CFDate::now().abs_time(), 0.05, 0, 0, refresh, &mut context);
    let run_loop = CFRunLoop::get_current();
    unsafe { run_loop.add_timer(&timer, kCFRunLoopCommonModes) };
    application.run();
    Ok(())
}

struct Indicator {
    panel: Retained<NSPanel>,
    label: Retained<NSTextField>,
    handle: Handle,
    phase: Option<Phase>,
    ticks: u8,
    width: f64,
}

impl Indicator {
    fn new(mtm: MainThreadMarker, handle: Handle) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(58.0, HEIGHT));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(true);
        panel.setHidesOnDeactivate(false);
        panel.setCanHide(false);
        panel.setExcludedFromWindowsMenu(true);
        panel.setLevel(NSFloatingWindowLevel);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::Transient,
        );

        let label = NSTextField::labelWithString(&NSString::from_str("● Ready"), mtm);
        label.setFrame(frame);
        label.setAlignment(NSTextAlignment::Center);
        label.setFont(Some(&NSFont::boldSystemFontOfSize(10.5)));
        panel.setContentView(Some(&label));

        Self {
            panel,
            label,
            handle,
            phase: None,
            ticks: POSITION_TICKS,
            width: 58.0,
        }
    }

    fn refresh(&mut self) {
        let phase = self.handle.get();
        if self.phase != Some(phase) {
            self.set_phase(phase);
            self.ticks = POSITION_TICKS;
        }

        self.ticks = self.ticks.saturating_add(1);
        if self.ticks < POSITION_TICKS {
            return;
        }
        self.ticks = 0;

        let Some(target) = cmux_window_frame() else {
            self.panel.orderOut(None);
            return;
        };
        let mtm = MainThreadMarker::new().expect("indicator callback runs on the main thread");
        let Some(primary_screen) = objc2_app_kit::NSScreen::screens(mtm).firstObject() else {
            self.panel.orderOut(None);
            return;
        };
        let origin = NSPoint::new(
            target.x + target.width - self.width - RIGHT_INSET,
            primary_screen.frame().size.height - target.y - HEIGHT - TOP_INSET,
        );
        self.panel
            .setFrame_display(NSRect::new(origin, NSSize::new(self.width, HEIGHT)), true);
        if !self.panel.isVisible() {
            self.panel.orderFrontRegardless();
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        let (text, width, color) = phase.presentation();
        self.phase = Some(phase);
        self.width = width;
        self.label.setStringValue(&NSString::from_str(text));
        self.label.setTextColor(Some(&color));
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, HEIGHT));
        self.label.setFrame(frame);
    }
}

extern "C" fn refresh(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    if info.is_null() {
        return;
    }
    autoreleasepool(|_| {
        // SAFETY: `info` points to the boxed indicator that lives until the app loop exits,
        // and this timer only runs on the main thread.
        unsafe { (&mut *info.cast::<Indicator>()).refresh() };
    });
}

struct WindowFrame {
    x: f64,
    y: f64,
    width: f64,
}

fn cmux_window_frame() -> Option<WindowFrame> {
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
