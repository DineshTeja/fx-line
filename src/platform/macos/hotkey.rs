use super::{daemon::Event, indicator::Handle as Indicator};
use crate::Result;
use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField, KeyCode,
};
use std::{
    io,
    sync::{Arc, Mutex, mpsc::Sender},
    time::{Duration, Instant},
};

const TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(15);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGRequestListenEventAccess() -> bool;
}

#[derive(Default)]
struct State {
    control_down: bool,
    function_down: bool,
    combo_down: bool,
    waiting_since: Option<Instant>,
}

impl State {
    fn modifier_changed(&mut self, key: u16, flags: CGEventFlags) -> Option<bool> {
        if key == KeyCode::CONTROL || key == KeyCode::RIGHT_CONTROL {
            self.control_down = flags.contains(CGEventFlags::CGEventFlagControl);
        } else if key == KeyCode::FUNCTION {
            self.function_down = if flags.contains(CGEventFlags::CGEventFlagSecondaryFn) {
                true
            } else {
                !self.function_down
            };
        }

        let pressed = self.control_down && self.function_down;
        if pressed == self.combo_down {
            return None;
        }
        self.combo_down = pressed;
        Some(pressed)
    }
}

pub fn request_access() {
    unsafe {
        CGRequestListenEventAccess();
    }
}

pub fn listen(sender: Sender<Event>, indicator: Indicator) -> Result<()> {
    let state = Arc::new(Mutex::new(State::default()));
    let modifier_state = Arc::clone(&state);
    let modifier_sender = sender.clone();
    let modifier_indicator = indicator.clone();
    let ready = sender.clone();

    let taps = CGEventTap::with_enabled(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged],
        move |_proxy, event_type, event| {
            handle(
                event_type,
                event,
                &modifier_state,
                &modifier_sender,
                &modifier_indicator,
            )
        },
        move || {
            CGEventTap::with_enabled(
                CGEventTapLocation::AnnotatedSession,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![CGEventType::KeyDown],
                move |_proxy, event_type, event| {
                    handle(event_type, event, &state, &sender, &indicator)
                },
                move || {
                    let _ = ready.send(Event::Ready);
                    CFRunLoop::run_current();
                },
            )
        },
    );

    if !matches!(taps, Ok(Ok(()))) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "fx-agent needs macOS Accessibility permission",
        )
        .into());
    }
    Ok(())
}

fn handle(
    event_type: CGEventType,
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
    indicator: &Indicator,
) -> CallbackResult {
    match event_type {
        CGEventType::FlagsChanged => modifiers_changed(event, state, sender, indicator),
        CGEventType::KeyDown => key_down(event, state, sender, indicator),
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            CFRunLoop::get_current().stop();
        }
        _ => {}
    }
    CallbackResult::Keep
}

fn modifiers_changed(
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
    indicator: &Indicator,
) {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let changed = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .modifier_changed(key, event.get_flags());

    match changed {
        Some(true) => {
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .waiting_since = None;
            let _ = sender.send(Event::Pressed);
            if !indicator.request_capture() {
                indicator.cancel_capture();
                let _ = sender.send(Event::Cancelled);
            }
        }
        Some(false) => {
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .waiting_since = Some(Instant::now());
            let _ = sender.send(Event::Released);
        }
        None => {}
    }
}

fn key_down(event: &CGEvent, state: &Mutex<State>, sender: &Sender<Event>, indicator: &Indicator) {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    let waiting = state
        .waiting_since
        .is_some_and(|started| started.elapsed() <= TRANSCRIPT_TIMEOUT);

    if waiting && key == KeyCode::ANSI_V && flags.contains(CGEventFlags::CGEventFlagCommand) {
        state.waiting_since = None;
        indicator.paste_capture();
        return;
    }
    if key != KeyCode::ESCAPE {
        return;
    }
    if !state.combo_down && !waiting {
        return;
    }
    state.combo_down = false;
    state.waiting_since = None;
    indicator.cancel_capture();
    let _ = sender.send(Event::Cancelled);
}

#[cfg(test)]
mod tests {
    use super::{KeyCode, State};
    use core_graphics::event::CGEventFlags;

    #[test]
    fn tracks_function_and_control_without_consuming_them() {
        let mut state = State::default();
        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::CGEventFlagSecondaryFn),
            None,
        );
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::CGEventFlagControl),
            Some(true),
        );
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::empty()),
            Some(false),
        );
    }

    #[test]
    fn handles_function_events_without_the_function_flag() {
        let mut state = State::default();
        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::empty()),
            None,
        );
        assert!(state.function_down);
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::CGEventFlagControl),
            Some(true),
        );
    }
}
