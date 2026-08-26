use crate::agent::Event;
use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField, KeyCode,
};
use std::{
    env, io,
    sync::{Arc, Mutex, mpsc::Sender},
    time::{Duration, Instant, SystemTime},
};

const PASTE_TIMEOUT: Duration = Duration::from_secs(15);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGRequestListenEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}

#[derive(Default)]
struct State {
    control_down: bool,
    function_down: bool,
    combo_down: bool,
    waiting_since: Option<Instant>,
    consuming_paste_key_up: bool,
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
        CGRequestPostEventAccess();
        CGRequestListenEventAccess();
    }
}

pub fn listen(sender: Sender<Event>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = Arc::new(Mutex::new(State::default()));
    let diagnostics = env::var_os("FX_AGENT_DIAGNOSTICS").is_some();
    let modifier_state = Arc::clone(&state);
    let modifier_sender = sender.clone();
    let ready_sender = sender.clone();

    let taps = CGEventTap::with_enabled(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::FlagsChanged],
        move |_proxy, event_type, event| {
            handle(
                event_type,
                event,
                &modifier_state,
                &modifier_sender,
                diagnostics,
            )
        },
        move || {
            CGEventTap::with_enabled(
                CGEventTapLocation::AnnotatedSession,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::KeyDown, CGEventType::KeyUp],
                move |_proxy, event_type, event| {
                    handle(event_type, event, &state, &sender, diagnostics)
                },
                move || {
                    let _ = ready_sender.send(Event::Ready);
                    CFRunLoop::run_current();
                },
            )
        },
    );

    match taps {
        Err(()) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not create the modifier event tap",
        ))?,
        Ok(Err(())) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not create the paste event tap",
        ))?,
        Ok(Ok(())) => {}
    }

    Ok(())
}

fn handle(
    event_type: CGEventType,
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
    diagnostics: bool,
) -> CallbackResult {
    match event_type {
        CGEventType::FlagsChanged => modifiers_changed(event, state, sender, diagnostics),
        CGEventType::KeyDown => key_down(event, state, sender, diagnostics),
        CGEventType::KeyUp => key_up(event, state),
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            CFRunLoop::get_current().stop();
            CallbackResult::Keep
        }
        _ => CallbackResult::Keep,
    }
}

fn modifiers_changed(
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
    diagnostics: bool,
) -> CallbackResult {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    if diagnostics
        && matches!(
            key,
            KeyCode::CONTROL | KeyCode::RIGHT_CONTROL | KeyCode::FUNCTION
        )
    {
        eprintln!(
            "fx-agent: modifier key={key} flags={:#x}",
            event.get_flags()
        );
    }
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());

    match state.modifier_changed(key, event.get_flags()) {
        Some(true) => {
            state.waiting_since = None;
            state.consuming_paste_key_up = false;
            let _ = sender.send(Event::Pressed(SystemTime::now()));
        }
        Some(false) => {
            state.waiting_since = Some(Instant::now());
            let _ = sender.send(Event::Released);
        }
        None => {}
    }

    CallbackResult::Keep
}

fn key_down(
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
    diagnostics: bool,
) -> CallbackResult {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());

    if diagnostics && key == KeyCode::ANSI_V && flags.contains(CGEventFlags::CGEventFlagCommand) {
        eprintln!(
            "fx-agent: paste event waiting={}",
            state.waiting_since.is_some()
        );
    }

    if key == KeyCode::ESCAPE && (state.combo_down || state.waiting_since.is_some()) {
        state.combo_down = false;
        state.waiting_since = None;
        state.consuming_paste_key_up = false;
        let _ = sender.send(Event::Cancelled);
        return CallbackResult::Keep;
    }

    let Some(started) = state.waiting_since else {
        return CallbackResult::Keep;
    };
    if started.elapsed() > PASTE_TIMEOUT {
        state.waiting_since = None;
        state.consuming_paste_key_up = false;
        let _ = sender.send(Event::Cancelled);
        return CallbackResult::Keep;
    }
    if key != KeyCode::ANSI_V || !flags.contains(CGEventFlags::CGEventFlagCommand) {
        return CallbackResult::Keep;
    }

    state.waiting_since = None;
    state.consuming_paste_key_up = true;
    drop(state);
    let _ = sender.send(Event::Paste);
    neutralize(event)
}

fn key_up(event: &CGEvent, state: &Mutex<State>) -> CallbackResult {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    if key != KeyCode::ANSI_V || !state.consuming_paste_key_up {
        return CallbackResult::Keep;
    }
    state.consuming_paste_key_up = false;
    neutralize(event)
}

fn neutralize(event: &CGEvent) -> CallbackResult {
    let event = event.clone();
    event.set_type(CGEventType::Null);
    event.set_flags(CGEventFlags::empty());
    CallbackResult::Replace(event)
}

#[cfg(test)]
mod tests {
    use super::{KeyCode, State, neutralize};
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventType, CallbackResult};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    #[test]
    fn state_starts_idle() {
        let state = State::default();
        assert!(!state.combo_down);
        assert!(state.waiting_since.is_none());
        assert!(!state.consuming_paste_key_up);
    }

    #[test]
    fn tracks_function_then_control() {
        let mut state = State::default();

        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::CGEventFlagSecondaryFn,),
            None
        );
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::CGEventFlagControl),
            Some(true)
        );
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::empty()),
            Some(false)
        );
    }

    #[test]
    fn tracks_control_then_function() {
        let mut state = State::default();

        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::CGEventFlagControl),
            None
        );
        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::CGEventFlagSecondaryFn,),
            Some(true)
        );
        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::CGEventFlagControl),
            Some(false)
        );
    }

    #[test]
    fn tracks_function_when_its_flag_is_missing() {
        let mut state = State::default();

        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::empty()),
            None
        );
        assert!(state.function_down);
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::CGEventFlagControl),
            Some(true)
        );
        assert_eq!(
            state.modifier_changed(KeyCode::CONTROL, CGEventFlags::empty()),
            Some(false)
        );
        assert_eq!(
            state.modifier_changed(KeyCode::FUNCTION, CGEventFlags::empty()),
            None
        );
        assert!(!state.function_down);
    }

    #[test]
    fn neutralized_paste_continues_without_a_key_event() {
        let source = CGEventSource::new(CGEventSourceStateID::Private).unwrap();
        let event = CGEvent::new_keyboard_event(source, KeyCode::ANSI_V, true).unwrap();
        event.set_flags(CGEventFlags::CGEventFlagCommand);
        let CallbackResult::Replace(event) = neutralize(&event) else {
            panic!("paste was not replaced");
        };
        assert_eq!(event.get_type() as u32, CGEventType::Null as u32);
        assert!(event.get_flags().is_empty());
    }
}
