use crate::agent::Event;
use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField, KeyCode,
};
use std::{
    io,
    process::Command,
    sync::{Arc, Mutex, mpsc::Sender},
    time::{Duration, Instant},
};

const PASTE_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TRANSCRIPT_BYTES: usize = 32 * 1024;

#[derive(Default)]
struct State {
    combo_down: bool,
    waiting_since: Option<Instant>,
}

pub fn listen(sender: Sender<Event>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state = Arc::new(Mutex::new(State::default()));

    CGEventTap::with_enabled(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
        move |_proxy, event_type, event| handle(event_type, event, &state, &sender),
        CFRunLoop::run_current,
    )
    .map_err(|()| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "fx-agent needs macOS Accessibility permission",
        )
    })?;

    Ok(())
}

fn handle(
    event_type: CGEventType,
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
) -> CallbackResult {
    match event_type {
        CGEventType::FlagsChanged => modifiers_changed(event, state, sender),
        CGEventType::KeyDown => key_down(event, state, sender),
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            // launchd immediately replaces us with a fresh event tap.
            std::process::exit(70)
        }
        _ => CallbackResult::Keep,
    }
}

fn modifiers_changed(
    event: &CGEvent,
    state: &Mutex<State>,
    sender: &Sender<Event>,
) -> CallbackResult {
    let flags = event.get_flags();
    let pressed = flags.contains(CGEventFlags::CGEventFlagControl)
        && flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());

    if pressed && !state.combo_down {
        state.combo_down = true;
        state.waiting_since = None;
        let _ = sender.send(Event::Pressed);
    } else if !pressed && state.combo_down {
        state.combo_down = false;
        state.waiting_since = Some(Instant::now());
        let _ = sender.send(Event::Released);
    }

    CallbackResult::Keep
}

fn key_down(event: &CGEvent, state: &Mutex<State>, sender: &Sender<Event>) -> CallbackResult {
    let key = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let flags = event.get_flags();
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());

    if key == KeyCode::ESCAPE && (state.combo_down || state.waiting_since.is_some()) {
        state.combo_down = false;
        state.waiting_since = None;
        let _ = sender.send(Event::Cancelled);
        return CallbackResult::Keep;
    }

    let Some(started) = state.waiting_since else {
        return CallbackResult::Keep;
    };
    if started.elapsed() > PASTE_TIMEOUT {
        state.waiting_since = None;
        let _ = sender.send(Event::Cancelled);
        return CallbackResult::Keep;
    }
    if key != KeyCode::ANSI_V || !flags.contains(CGEventFlags::CGEventFlagCommand) {
        return CallbackResult::Keep;
    }

    state.waiting_since = None;
    drop(state);
    match transcript() {
        Ok(text) if !text.trim().is_empty() => {
            let _ = sender.send(Event::Transcript(text));
        }
        _ => {
            let _ = sender.send(Event::Cancelled);
        }
    }
    CallbackResult::Drop
}

fn transcript() -> io::Result<String> {
    let output = Command::new("/usr/bin/pbpaste").output()?;
    if !output.status.success() {
        return Err(io::Error::other("could not read Wispr transcript"));
    }
    if output.stdout.len() > MAX_TRANSCRIPT_BYTES {
        return Err(io::Error::other("Wispr transcript is too large"));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn state_starts_idle() {
        let state = State::default();
        assert!(!state.combo_down);
        assert!(state.waiting_since.is_none());
    }
}
