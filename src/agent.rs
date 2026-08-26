use crate::{
    cmux::{Action, Cmux, Context, Status},
    context, fx, output,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    io,
    path::Path,
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::Duration,
};

const PLAN_ATTEMPTS: usize = 2;
const TRANSCRIPT_TIMEOUT: Duration = Duration::from_secs(15);
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
    Pressed,
    Released,
    Transcript(String),
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
    let (sender, receiver) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("fx-agent-worker".into())
        .stack_size(256 * 1024)
        .spawn(move || worker(receiver))?;

    #[cfg(target_os = "macos")]
    {
        let result = crate::hotkey::listen(sender);
        if result.is_err() {
            let _ = worker.join();
            let cmux = Cmux::new();
            if let Ok(context) = cmux.context() {
                let _ = cmux.status(&context, Status::Off);
            }
        }
        result
    }

    #[cfg(not(target_os = "macos"))]
    {
        drop(sender);
        let _ = worker.join();
        Err(io::Error::other("fx-agent requires macOS").into())
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

fn worker(receiver: Receiver<Event>) {
    let cmux = Cmux::new();
    if let Ok(context) = cmux.context() {
        let _ = cmux.status(&context, Status::Ready);
    }
    let mut context = None;
    let mut awaiting_transcript = false;

    loop {
        let event = if awaiting_transcript {
            match receiver.recv_timeout(TRANSCRIPT_TIMEOUT) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => {
                    awaiting_transcript = false;
                    if let Some(context) = context.take() {
                        let _ = cmux.status(&context, Status::Ready);
                    }
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
            Some(Event::Pressed) => {
                awaiting_transcript = false;
                context = cmux.context().ok();
                if let Some(context) = &context {
                    let _ = cmux.status(context, Status::Listening);
                }
            }
            Some(Event::Released) => {
                awaiting_transcript = true;
                if let Some(context) = &context {
                    let _ = cmux.status(context, Status::Transcribing);
                }
            }
            Some(Event::Transcript(request)) => {
                awaiting_transcript = false;
                let Some(context) = context.take() else {
                    continue;
                };
                if let Err(error) = run_with_context(&cmux, &context, request.trim()) {
                    let _ = cmux.status(&context, Status::Error);
                    let _ = cmux.notify(&context, &summary(&error.to_string()));
                    thread::sleep(Duration::from_millis(700));
                    let _ = cmux.status(&context, Status::Ready);
                }
            }
            Some(Event::Cancelled) => {
                awaiting_transcript = false;
                if let Some(context) = context.take() {
                    let _ = cmux.status(&context, Status::Ready);
                }
            }
            None => {}
        }
    }
}

fn run_with_context(cmux: &Cmux, context: &Context, request: &str) -> Result<String> {
    if request.is_empty() {
        return Err(io::Error::other("Wispr returned an empty transcript").into());
    }
    cmux.status(context, Status::Working)?;

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

    cmux.status(context, Status::Done)?;
    cmux.notify(context, &result)?;
    thread::sleep(Duration::from_millis(700));
    cmux.status(context, Status::Ready)?;
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
