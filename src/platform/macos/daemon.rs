use super::{
    hotkey,
    indicator::{Handle as Indicator, Phase},
};
use crate::{Result, agent, cmux::Cmux};
use std::{
    env,
    fmt::Display,
    fs::OpenOptions,
    io,
    io::Write,
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
const PERMISSION_RETRY: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(super) enum Event {
    Ready,
    Unavailable,
    Pressed,
    Released,
    Transcript(String),
    Cancelled,
}

pub(crate) fn run() -> Result<()> {
    let indicator = Indicator::default();
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("fx-agent-worker".into())
        .stack_size(256 * 1024)
        .spawn({
            let indicator = indicator.clone();
            move || worker(receiver, indicator)
        })?;
    thread::Builder::new()
        .name("fx-agent-hotkey".into())
        .stack_size(256 * 1024)
        .spawn({
            let indicator = indicator.clone();
            let sender = sender.clone();
            move || listen(sender, indicator)
        })?;
    super::indicator::run(indicator, sender)?;
    Ok(())
}

fn listen(sender: mpsc::Sender<Event>, indicator: Indicator) {
    let mut reported = false;
    loop {
        match hotkey::listen(sender.clone(), indicator.clone()) {
            Ok(()) => {
                reported = false;
                thread::sleep(Duration::from_millis(100));
            }
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied) =>
            {
                if !reported {
                    report(error);
                    hotkey::request_access();
                    reported = true;
                }
                let _ = sender.send(Event::Unavailable);
                thread::sleep(PERMISSION_RETRY);
            }
            Err(error) => {
                report(error);
                let _ = sender.send(Event::Unavailable);
                thread::sleep(PERMISSION_RETRY);
            }
        }
    }
}

fn worker(receiver: Receiver<Event>, indicator: Indicator) {
    let cmux = Cmux::new();
    let mut context = None;
    let mut awaiting_transcript = false;
    let _ = cmux.clear_agent_statuses();

    loop {
        let event = if awaiting_transcript {
            match receiver.recv_timeout(TRANSCRIPT_TIMEOUT) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => {
                    awaiting_transcript = false;
                    context = None;
                    indicator.cancel_capture();
                    indicator.set(Phase::Ready);
                    None
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(event) => Some(event),
                Err(_) => break,
            }
        };

        match event {
            Some(Event::Ready) => indicator.set(Phase::Ready),
            Some(Event::Unavailable) => indicator.set(Phase::Off),
            Some(Event::Pressed) => {
                awaiting_transcript = false;
                context = match cmux.context() {
                    Ok(context) => Some(context),
                    Err(error) => {
                        report(format_args!("CMUX context capture failed: {error}"));
                        None
                    }
                };
                indicator.set(Phase::Listening);
            }
            Some(Event::Released) => {
                awaiting_transcript = true;
                indicator.set(Phase::Transcribing);
            }
            Some(Event::Transcript(request)) => {
                awaiting_transcript = false;
                let context = context.take().or_else(|| match cmux.context() {
                    Ok(context) => Some(context),
                    Err(error) => {
                        report(format_args!("CMUX context retry failed: {error}"));
                        None
                    }
                });
                let result = match context.as_ref() {
                    Some(context) => {
                        indicator.set(Phase::Working);
                        agent::run_with_context(&cmux, context, request.trim()).map(|_| ())
                    }
                    None => {
                        Err(io::Error::other("could not capture the focused CMUX workspace").into())
                    }
                };
                if let Err(error) = result {
                    report(format_args!("request failed: {error}"));
                    indicator.set(Phase::Error);
                    if let Some(context) = &context {
                        let _ = cmux.notify(context, &agent::summary(&error.to_string()));
                    }
                } else {
                    indicator.set(Phase::Done);
                }
                thread::sleep(Duration::from_millis(700));
                indicator.set(Phase::Ready);
            }
            Some(Event::Cancelled) => {
                awaiting_transcript = false;
                context = None;
                indicator.cancel_capture();
                indicator.set(Phase::Ready);
            }
            None => {}
        }
    }
}

fn report(error: impl Display) {
    eprintln!("fx-agent: {error}");
    let Some(home) = env::var_os("HOME") else {
        return;
    };
    let path = Path::new(&home).join("Library/Application Support/fx-line/agent.log");
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let _ = writeln!(log, "{timestamp:.3} fx-agent: {error}");
}
