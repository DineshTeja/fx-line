use super::intent;
use crate::{
    Result,
    cmux::{Action, Cmux, Context},
    model, project,
};
use serde::{Deserialize, Serialize};
use std::io;

const ATTEMPTS: usize = 2;
const RULES: &str = r#"Return only JSON: {"actions":[],"fallback":false,"message":"short result"}.
Every request needs actions or fallback=true. Never use tools.
Actions: open_browser(url,[direction]); new_terminal(direction); focus_pane(pane); swap_panes(pane,target); resize_pane(pane,direction,amount); move_surface(surface,pane); split_surface(surface,direction); new_workspace([name],[cwd]); select_workspace(workspace); rename_workspace(name,[workspace]); rename_tab(name,[surface]); open(target); sidebar(action); flash; close_surface([surface]); close_workspace([workspace]).
Directions: left/right/up/down. Sidebar: show/hide/toggle/focus. Use only context refs. Never close unless asked.
Use pane and surface refs only from cmux.tree. Use cmux.workspaces refs only to select a workspace.
Prefer direct actions whenever the schema can satisfy the request. Use open_browser for a site, URL, or site name by itself. Use fallback=true, with no actions, only for filesystem or shell work, interaction inside an existing web page, or CMUX work outside the schema.
Example for open GitHub and Google: {"actions":[{"kind":"open_browser","url":"https://github.com","direction":"right"},{"kind":"open_browser","url":"https://google.com","direction":"down"}],"fallback":false,"message":"Opened two browsers"}."#;

#[derive(Debug, Deserialize, Serialize)]
pub struct Plan {
    #[serde(default)]
    pub(super) actions: Vec<Action>,
    #[serde(default)]
    pub(super) fallback: bool,
    #[serde(default)]
    pub(super) message: String,
}

pub(super) fn create(request: &str, context: &Context) -> Result<Plan> {
    if let Some(intent) = intent::parse(request) {
        let plan = Plan {
            actions: vec![intent.action],
            fallback: false,
            message: intent.message,
        };
        validate_actions(&plan, request, context)?;
        return Ok(plan);
    }

    let mut prompt = prompt(request, context);
    let mut last_error = None;
    for _ in 0..ATTEMPTS {
        match model::plan(&prompt).and_then(|text| validate(&text, request, context)) {
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

fn prompt(request: &str, context: &Context) -> String {
    let directory = context.cwd.as_deref().map(project::directory);
    let context = serde_json::json!({
        "cmux": context,
        "directory": directory,
        "request": request,
    });

    format!("{RULES}\n{context}")
}

fn validate(text: &str, request: &str, context: &Context) -> Result<Plan> {
    let mut plan: Plan = model::response::json(text)?;
    if plan.actions.len() > 12 {
        return Err(io::Error::other("model returned too many CMUX actions").into());
    }
    if plan.actions.is_empty() {
        plan.fallback = true;
    } else if plan.fallback {
        return Err(io::Error::other("model mixed direct actions with fallback").into());
    }
    validate_actions(&plan, request, context)?;
    Ok(plan)
}

fn validate_actions(plan: &Plan, request: &str, context: &Context) -> Result<()> {
    let cmux = Cmux::new();
    for action in &plan.actions {
        cmux.validate(context, request, action)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Plan;

    #[test]
    fn defaults_optional_fields() {
        let plan: Plan = serde_json::from_str(r#"{"actions":[]}"#).unwrap();
        assert!(!plan.fallback);
        assert!(plan.message.is_empty());
    }
}
