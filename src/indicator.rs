use crate::{agent::Event, capture};
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
    NSFloatingWindowLevel, NSFont, NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration,
    NSImageView, NSPanel, NSTextAlignment, NSTextField, NSView, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    ffi::c_void,
    io,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::Sender,
    },
};

const CMUX_BUNDLE_ID: &str = "com.cmuxterm.app";
const HEIGHT: f64 = 28.0;
const RIGHT_INSET: f64 = 12.0;
const TOP_INSET: f64 = 0.0;
const FONT_SIZE: f64 = 12.5;
const ICON_FRAME: f64 = 13.0;
const ICON_SPACING: f64 = 5.0;
const HORIZONTAL_PADDING: f64 = 2.0;
const TEXT_VERTICAL_NUDGE: f64 = -1.5;
const REFRESH_INTERVAL: f64 = 0.02;
const POSITION_TICKS: u8 = 20;

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

    fn presentation(self) -> Presentation {
        match self {
            Self::Off => Presentation::new("Off", "circle", 7.0, Tone::Muted),
            Self::Ready => Presentation::new("Ready", "circle.fill", 7.0, Tone::Muted),
            Self::Listening => Presentation::new("Listening", "waveform", 11.0, Tone::Red),
            Self::Transcribing => Presentation::new("Transcribing", "ellipsis", 10.0, Tone::Orange),
            Self::Working => Presentation::new("Working", "sparkles", 10.0, Tone::Blue),
            Self::Done => Presentation::new("Done", "checkmark", 10.0, Tone::Green),
            Self::Error => Presentation::new("Error", "exclamationmark", 10.0, Tone::Red),
        }
    }
}

struct Presentation {
    text: &'static str,
    symbol: &'static str,
    icon_size: f64,
    tone: Tone,
}

impl Presentation {
    const fn new(text: &'static str, symbol: &'static str, icon_size: f64, tone: Tone) -> Self {
        Self {
            text,
            symbol,
            icon_size,
            tone,
        }
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Muted,
    Red,
    Orange,
    Blue,
    Green,
}

impl Tone {
    fn color(self) -> Retained<NSColor> {
        match self {
            Self::Muted => NSColor::tertiaryLabelColor(),
            Self::Red => NSColor::systemRedColor(),
            Self::Orange => NSColor::systemOrangeColor(),
            Self::Blue => NSColor::systemBlueColor(),
            Self::Green => NSColor::systemGreenColor(),
        }
    }
}

#[derive(Clone, Default)]
pub struct Handle {
    phase: Arc<AtomicU8>,
    capture: capture::Handle,
}

impl Handle {
    pub fn set(&self, phase: Phase) {
        self.phase.store(phase as u8, Ordering::Release);
    }

    fn get(&self) -> Phase {
        Phase::from_raw(self.phase.load(Ordering::Acquire))
    }

    pub fn request_capture(&self) -> bool {
        self.capture.request()
    }

    pub fn cancel_capture(&self) {
        self.capture.cancel();
    }

    pub fn paste_capture(&self) {
        self.capture.paste();
    }
}

pub fn run(handle: Handle, sender: Sender<Event>) -> io::Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("the CMUX indicator must run on the main thread"))?;
    let application = NSApplication::sharedApplication(mtm);
    application.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut indicator = Box::new(Indicator::new(mtm, handle, sender));
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: (&mut *indicator as *mut Indicator).cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    let timer = CFRunLoopTimer::new(
        CFDate::now().abs_time(),
        REFRESH_INTERVAL,
        0,
        0,
        refresh,
        &mut context,
    );
    let run_loop = CFRunLoop::get_current();
    unsafe { run_loop.add_timer(&timer, kCFRunLoopCommonModes) };
    application.run();
    Ok(())
}

struct Indicator {
    panel: Retained<NSPanel>,
    icon: Retained<NSImageView>,
    label: Retained<NSTextField>,
    capture: capture::Target,
    handle: Handle,
    phase: Option<Phase>,
    ticks: u8,
    width: f64,
    target: Option<WindowFrame>,
}

impl Indicator {
    fn new(mtm: MainThreadMarker, handle: Handle, sender: Sender<Event>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, HEIGHT));
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

        let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
        let image = symbol("circle.fill").expect("macOS provides the circle SF Symbol");
        let icon = NSImageView::imageViewWithImage(&image, mtm);
        content.addSubview(&icon);

        let label = NSTextField::labelWithString(&NSString::from_str("Ready"), mtm);
        label.setAlignment(NSTextAlignment::Left);
        label.setFont(Some(&NSFont::systemFontOfSize_weight(FONT_SIZE, unsafe {
            NSFontWeightSemibold
        })));
        content.addSubview(&label);
        panel.setContentView(Some(&content));

        Self {
            panel,
            icon,
            label,
            capture: capture::Target::new(mtm, sender),
            handle,
            phase: None,
            ticks: POSITION_TICKS,
            width: 64.0,
            target: None,
        }
    }

    fn refresh(&mut self) {
        self.capture.refresh(&self.handle.capture);
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

        if let Some(target) = cmux_window_frame() {
            self.target = Some(target);
        }
        let Some(target) = self
            .target
            .as_ref()
            .filter(|_| cmux_is_frontmost() || self.capture.is_active() || phase != Phase::Ready)
        else {
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
        let presentation = phase.presentation();
        let text_color = match phase {
            Phase::Off | Phase::Ready => NSColor::secondaryLabelColor(),
            _ => NSColor::labelColor(),
        };
        self.phase = Some(phase);
        self.label
            .setStringValue(&NSString::from_str(presentation.text));
        self.label.setTextColor(Some(&text_color));
        self.label.sizeToFit();

        if let Some(image) = symbol(presentation.symbol) {
            self.icon.setImage(Some(&image));
        }
        let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            presentation.icon_size,
            unsafe { NSFontWeightSemibold },
        );
        self.icon.setSymbolConfiguration(Some(&configuration));
        self.icon
            .setContentTintColor(Some(&presentation.tone.color()));

        let icon_origin = NSPoint::new(HORIZONTAL_PADDING, ((HEIGHT - ICON_FRAME) / 2.0).round());
        self.icon.setFrame(NSRect::new(
            icon_origin,
            NSSize::new(ICON_FRAME, ICON_FRAME),
        ));

        let label_size = self.label.frame().size;
        let label_origin = NSPoint::new(
            HORIZONTAL_PADDING + ICON_FRAME + ICON_SPACING,
            ((HEIGHT - label_size.height) / 2.0 + TEXT_VERTICAL_NUDGE).round(),
        );
        self.label.setFrameOrigin(label_origin);
        self.width = (label_origin.x + label_size.width + HORIZONTAL_PADDING).ceil();
    }
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
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

#[derive(Clone, Copy)]
struct WindowFrame {
    x: f64,
    y: f64,
    width: f64,
}

fn cmux_is_frontmost() -> bool {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|application| application.bundleIdentifier())
        .is_some_and(|bundle| bundle.to_string() == CMUX_BUNDLE_ID)
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
