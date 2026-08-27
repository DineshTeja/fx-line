mod intent;
mod planner;

pub use planner::Plan;

use crate::{
    Result,
    cmux::{Action, Cmux, Context},
    model, project,
};
use std::{io, path::Path};

pub fn run_daemon() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::daemon::run()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(io::Error::other("fx-agent requires macOS").into())
    }
}

pub fn plan(request: &str) -> Result<Plan> {
    let context = Cmux::new().context()?;
    planner::create(request, &context)
}

pub fn run_request(request: &str) -> Result<String> {
    let cmux = Cmux::new();
    let context = cmux.context()?;
    run_with_context(&cmux, &context, request)
}

pub fn install(binary: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::service::install(binary)?)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = binary;
        Err(io::Error::other("fx-agent requires macOS").into())
    }
}

pub fn uninstall() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::service::uninstall()?)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(io::Error::other("fx-agent requires macOS").into())
    }
}

pub fn is_running() -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::service::is_running()?)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(io::Error::other("fx-agent requires macOS").into())
    }
}

pub(crate) fn run_with_context(cmux: &Cmux, context: &Context, request: &str) -> Result<String> {
    if request.is_empty() {
        return Err(io::Error::other("the request is empty").into());
    }
    let plan = planner::create(request, context)?;
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

fn run_task(task: &str, context: &Context) -> Result<String> {
    let cwd = context
        .cwd
        .as_deref()
        .ok_or_else(|| io::Error::other("the focused CMUX surface has no working directory"))?;
    let prompt = task_prompt(task, cwd, context);
    model::run_agent(&prompt, cwd)
}

fn task_prompt(task: &str, cwd: &Path, context: &Context) -> String {
    let context = serde_json::json!({
        "request": task,
        "cwd": cwd,
        "directory": project::directory(cwd),
        "cmux": context,
    });
    format!(
        "Complete this request now in cwd. Work quietly in the background, use tools as needed, and keep the final response concise. The `cmux` CLI is available for workspace, pane, terminal, browser, notification, and sidebar actions. Do not ask a follow-up when a safe reasonable interpretation exists.\n{context}"
    )
}

pub(crate) fn summary(text: &str) -> String {
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
    use super::summary;

    #[test]
    fn notification_summary_is_short_and_single_line() {
        assert_eq!(summary("first\n\n second"), "first second");
        assert!(summary(&"x".repeat(400)).chars().count() <= 281);
    }
}
