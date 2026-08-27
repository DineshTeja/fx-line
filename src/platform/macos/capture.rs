use super::daemon::Event;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSApplicationActivationOptions, NSBackingStoreType, NSColor,
    NSFloatingWindowLevel, NSPanel, NSRunningApplication, NSTextField, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};

const CMUX_BUNDLE_ID: &str = "com.cmuxterm.app";
const ACK_TIMEOUT: Duration = Duration::from_millis(100);
const LIFETIME: Duration = Duration::from_secs(20);
const CLEANUP_DELAY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Shared {
    request: AtomicU64,
    ready: AtomicU64,
    paste: AtomicU64,
    cancelled: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Handle(Arc<Shared>);

impl Handle {
    pub fn request(&self) -> bool {
        self.0.cancelled.store(false, Ordering::Release);
        let request = self.0.request.fetch_add(1, Ordering::AcqRel) + 1;
        let deadline = Instant::now() + ACK_TIMEOUT;
        while Instant::now() < deadline {
            if self.0.ready.load(Ordering::Acquire) >= request {
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        false
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
    }

    pub fn paste(&self) {
        self.0.paste.fetch_add(1, Ordering::AcqRel);
    }
}

pub struct Target {
    panel: Retained<NSPanel>,
    field: Retained<NSTextField>,
    sender: Sender<Event>,
    request: u64,
    paste: u64,
    started: Option<Instant>,
    completed: Option<Instant>,
}

impl Target {
    pub fn new(mtm: MainThreadMarker, sender: Sender<Event>) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(8.0, 8.0));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setAlphaValue(0.01);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
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
        panel.setFloatingPanel(true);
        panel.setBecomesKeyOnlyIfNeeded(false);

        let field = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);
        field.setEditable(true);
        field.setSelectable(true);
        field.setBordered(false);
        field.setBezeled(false);
        field.setDrawsBackground(false);
        panel.setContentView(Some(&field));

        Self {
            panel,
            field,
            sender,
            request: 0,
            paste: 0,
            started: None,
            completed: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.started.is_some()
    }

    pub fn refresh(&mut self, handle: &Handle) {
        let request = handle.0.request.load(Ordering::Acquire);
        if request > self.request {
            self.request = request;
            self.begin();
            handle.0.ready.store(request, Ordering::Release);
        }

        if handle.0.cancelled.load(Ordering::Acquire) {
            self.end();
            return;
        }
        let Some(started) = self.started else {
            return;
        };

        let paste = handle.0.paste.load(Ordering::Acquire);
        if paste > self.paste {
            self.paste = paste;
            self.paste();
        }

        if self.completed.is_none() {
            let text = self.field.stringValue().to_string();
            if !text.trim().is_empty() {
                let _ = self.sender.send(Event::Transcript(text));
                self.completed = Some(Instant::now());
            } else if started.elapsed() >= LIFETIME {
                self.end();
            }
        } else if self
            .completed
            .is_some_and(|completed| completed.elapsed() >= CLEANUP_DELAY)
        {
            self.end();
        }
    }

    fn begin(&mut self) {
        self.field.setStringValue(&NSString::new());
        self.completed = None;
        self.started = Some(Instant::now());

        let application = NSApplication::sharedApplication(
            MainThreadMarker::new().expect("capture runs on the main thread"),
        );
        #[allow(deprecated)]
        application.activateIgnoringOtherApps(true);
        self.panel.makeKeyAndOrderFront(None);
        let _ = self.panel.makeFirstResponder(Some(&self.field));
        application.setAccessibilityFrontmost(true);
        unsafe {
            application.setAccessibilityFocusedWindow(Some(&self.panel));
            application.setAccessibilityApplicationFocusedUIElement(Some(&self.field));
        }
        self.field.setAccessibilityFocused(true);
        application.activate();
    }

    fn paste(&self) {
        if !self.field.stringValue().is_empty() {
            return;
        }
        let editor = unsafe { self.panel.fieldEditor_forObject(true, Some(&self.field)) };
        if let Some(editor) = editor {
            unsafe { editor.paste(None) };
        }
    }

    fn end(&mut self) {
        if self.started.take().is_none() {
            return;
        }
        self.completed = None;
        self.field.setStringValue(&NSString::new());
        self.panel.orderOut(None);
        activate_cmux();
    }
}

fn activate_cmux() {
    let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(CMUX_BUNDLE_ID),
    );
    if let Some(application) = applications.firstObject() {
        let current = NSApplication::sharedApplication(
            MainThreadMarker::new().expect("CMUX activation runs on the main thread"),
        );
        current.yieldActivationToApplication(&application);
        application.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
    }
}
