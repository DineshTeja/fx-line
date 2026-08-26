#[cfg(target_os = "macos")]
use crate::indicator::{Handle as Indicator, Phase};
use crate::{
    cmux::{Action, Cmux, Context},
    context, fx, output,
};
use serde::{Deserialize, Serialize};
use std::{
    env,
    error::Error,
    fmt::Display,
    fs::OpenOptions,
    io,
    io::Write,
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const PLAN_ATTEMPTS: usize = 2;
const TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
const PERMISSION_RETRY: Duration = Duration::from_secs(1);
const PLAN_RULES: &str = r#"Return only JSON: {"actions":[],"fallback":false,"message":"short result"}.
Every request needs actions or fallback=true. Never use tools.
Actions: open_browser(url,[direction]); new_terminal(direction); focus_pane(pane); swap_panes(pane,target); resize_pane(pane,direction,amount); move_surface(surface,pane); split_surface(surface,direction); new_workspace([name],[cwd]); select_workspace(workspace); rename_workspace(name,[workspace]); rename_tab(name,[surface]); open(target); sidebar(action); flash; close_surface([surface]); close_workspace([workspace]).
Directions: left/right/up/down. Sidebar: show/hide/toggle/focus. Use only context refs. Never close unless asked.
Use pane and surface refs only from cmux.tree. Use cmux.workspaces refs only to select a workspace.
Use fallback=true, with no actions, for filesystem, shell, browser interaction, or CMUX work outside the schema.
Example for open GitHub and Google: {"actions":[{"kind":"open_browser","url":"https://github.com","direction":"right"},{"kind":"open_browser","url":"https://google.com","direction":"down"}],"fallback":false,"message":"Opened two browsers"}."#;

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Debug)]
pub enum Event {
    Ready,
    Unavailable,
    Pressed(SystemTime),
    Released,
    Paste,
    Cancelled,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Plan {
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub fallback: bool,
    #[serde(default)]
    pub message: String,
}

pub fn run_daemon() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
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
            .spawn(move || listen(sender))?;
        crate::indicator::run(indicator)?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(io::Error::other("fx-agent requires macOS").into())
    }
}

#[cfg(target_os = "macos")]
fn listen(sender: mpsc::Sender<Event>) {
    let mut reported = false;
    loop {
        match crate::hotkey::listen(sender.clone()) {
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
                    crate::hotkey::request_access();
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

pub fn plan(request: &str) -> Result<Plan> {
    let cmux = Cmux::new();
    let context = cmux.context()?;
    create_plan(request, &context)
}

pub fn run_request(request: &str) -> Result<String> {
    let cmux = Cmux::new();
    let context = cmux.context()?;
    run_with_context(&cmux, &context, request)
}

#[cfg(target_os = "macos")]
fn worker(receiver: Receiver<Event>, indicator: Indicator) {
    let cmux = Cmux::new();
    let mut context = None;
    let mut capture = None;
    let mut awaiting_transcript = false;
    let _ = cmux.clear_agent_statuses();

    loop {
        let event = if awaiting_transcript {
            match receiver.recv_timeout(TRANSCRIPT_TIMEOUT) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => {
                    awaiting_transcript = false;
                    context = None;
                    capture = None;
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
            Some(Event::Ready) => {
                indicator.set(Phase::Ready);
            }
            Some(Event::Unavailable) => {
                indicator.set(Phase::Off);
            }
            Some(Event::Pressed(pressed_at)) => {
                awaiting_transcript = false;
                capture = Some(crate::wispr::capture(pressed_at));
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
            Some(Event::Paste) => {
                awaiting_transcript = false;
                indicator.dismiss_wispr_notification();
                let context = context.take().or_else(|| match cmux.context() {
                    Ok(context) => Some(context),
                    Err(error) => {
                        report(format_args!("CMUX context retry failed: {error}"));
                        None
                    }
                });
                let result = match (context.as_ref(), capture.take()) {
                    (Some(context), Some(capture)) => {
                        crate::wispr::transcript(capture).and_then(|request| {
                            indicator.set(Phase::Working);
                            run_with_context(&cmux, context, request.trim()).map(|_| ())
                        })
                    }
                    _ => {
                        Err(io::Error::other("could not capture the focused CMUX workspace").into())
                    }
                };
                if let Err(error) = result {
                    report(format_args!("request failed: {error}"));
                    indicator.set(Phase::Error);
                    if let Some(context) = &context {
                        let _ = cmux.notify(context, &summary(&error.to_string()));
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
                capture = None;
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

fn run_with_context(cmux: &Cmux, context: &Context, request: &str) -> Result<String> {
    if request.is_empty() {
        return Err(io::Error::other("Wispr returned an empty transcript").into());
    }
    let plan = create_plan(request, context)?;
    for action in &plan.actions {
        cmux.execute(context, request, action)?;
    }
    let closed_workspace = plan.actions.iter().any(|action| {
        matches!(
            action,
            Action::CloseWorkspace { workspace }
                if workspace.as_deref().is_none_or(|workspace| workspace == context.workspace)
        )
    });

    let task_result = plan
        .fallback
        .then(|| run_task(request, context))
        .transpose()?;
    let result = task_result
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| (!plan.message.trim().is_empty()).then_some(plan.message.as_str()))
        .unwrap_or("Done");
    let result = summary(result);

    if closed_workspace {
        return Ok(result);
    }

    cmux.notify(context, &result)?;
    Ok(result)
}

fn create_plan(request: &str, context: &Context) -> Result<Plan> {
    let mut prompt = plan_prompt(request, context);
    let mut last_error = None;
    for _ in 0..PLAN_ATTEMPTS {
        match fx::plan(&prompt).and_then(|text| validate_plan(&text, request, context)) {
            Ok(plan) => return Ok(plan),
            Err(error) => {
                prompt.push_str(
                    "\nThe previous response was invalid. Follow the action schema exactly.",
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.expect("at least one plan attempt"))
}

fn plan_prompt(request: &str, context: &Context) -> String {
    let directory = context.cwd.as_deref().map(context::directory);
    let context = serde_json::json!({
        "cmux": context,
        "directory": directory,
        "request": request,
    });

    format!("{PLAN_RULES}\n{context}")
}

fn validate_plan(text: &str, request: &str, context: &Context) -> Result<Plan> {
    let mut plan: Plan = output::json(text)?;
    if plan.actions.len() > 12 {
        return Err(io::Error::other("model returned too many CMUX actions").into());
    }
    if plan.actions.is_empty() {
        plan.fallback = true;
    } else if plan.fallback {
        return Err(io::Error::other("model mixed direct actions with fallback").into());
    }
    let cmux = Cmux::new();
    for action in &plan.actions {
        cmux.validate(context, request, action)?;
    }
    Ok(plan)
}

fn run_task(task: &str, context: &Context) -> Result<String> {
    let cwd = context
        .cwd
        .as_deref()
        .ok_or_else(|| io::Error::other("the focused CMUX surface has no working directory"))?;
    let prompt = task_prompt(task, cwd, context);
    fx::run_agent(&prompt, cwd)
}

fn task_prompt(task: &str, cwd: &Path, context: &Context) -> String {
    let context = serde_json::json!({
        "request": task,
        "cwd": cwd,
        "directory": context::directory(cwd),
        "cmux": context,
    });
    format!(
        "Complete this request now in cwd. Work quietly in the background, use tools as needed, and keep the final response concise. The `cmux` CLI is available for workspace, pane, terminal, browser, notification, and sidebar actions. Do not ask a follow-up when a safe reasonable interpretation exists.\n{context}"
    )
}

fn summary(text: &str) -> String {
    let text = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = text.chars();
    let result = chars.by_ref().take(280).collect::<String>();
    if chars.next().is_some() {
        format!("{result}…")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{Plan, summary};

    #[test]
    fn plan_defaults_optional_fields() {
        let plan: Plan = serde_json::from_str(r#"{"actions":[]}"#).unwrap();
        assert!(!plan.fallback);
        assert!(plan.message.is_empty());
    }

    #[test]
    fn notification_summary_is_short_and_single_line() {
        assert_eq!(summary("first\n\n second"), "first second");
        assert!(summary(&"x".repeat(400)).chars().count() <= 281);
    }
}
