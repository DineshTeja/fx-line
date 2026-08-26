use super::{Context, actions::Action};
use std::io;

pub(super) fn validate(context: &Context, request: &str, action: &Action) -> io::Result<()> {
    match action {
        Action::CloseSurface { surface } => {
            destructive(request)?;
            reference(
                context,
                surface.as_deref().unwrap_or(&context.surface),
                "surface",
            )?;
        }
        Action::CloseWorkspace { workspace } => {
            destructive(request)?;
            reference(
                context,
                workspace.as_deref().unwrap_or(&context.workspace),
                "workspace",
            )?;
        }
        Action::Flash | Action::NewTerminal { .. } | Action::Sidebar { .. } => {}
        Action::FocusPane { pane } => reference(context, pane, "pane")?,
        Action::MoveSurface { pane, surface } => {
            reference(context, pane, "pane")?;
            reference(context, surface, "surface")?;
        }
        Action::NewWorkspace { cwd, name } => {
            if let Some(cwd) = cwd
                && !cwd.is_absolute()
            {
                return Err(io::Error::other("workspace cwd must be absolute"));
            }
            if let Some(name) = name {
                text(name, 128)?;
            }
        }
        Action::Open { target } => text(target, 2_048)?,
        Action::OpenBrowser { url, .. } => {
            text(url, 2_048)?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(io::Error::other(
                    "browser URL must start with http:// or https://",
                ));
            }
        }
        Action::RenameTab { name, surface } => {
            text(name, 128)?;
            reference(
                context,
                surface.as_deref().unwrap_or(&context.surface),
                "surface",
            )?;
        }
        Action::RenameWorkspace { name, workspace } => {
            text(name, 128)?;
            reference(
                context,
                workspace.as_deref().unwrap_or(&context.workspace),
                "workspace",
            )?;
        }
        Action::ResizePane { amount, pane, .. } => {
            if !(1..=100).contains(amount) {
                return Err(io::Error::other(
                    "pane resize amount must be between 1 and 100",
                ));
            }
            reference(context, pane, "pane")?;
        }
        Action::SelectWorkspace { workspace } => reference(context, workspace, "workspace")?,
        Action::SplitSurface { surface, .. } => reference(context, surface, "surface")?,
        Action::SwapPanes { pane, target } => {
            reference(context, pane, "pane")?;
            reference(context, target, "pane")?;
        }
    }
    Ok(())
}

fn reference(context: &Context, value: &str, kind: &str) -> io::Result<()> {
    let prefix = format!("{kind}:");
    let valid = value.strip_prefix(&prefix).is_some_and(|number| {
        !number.is_empty() && number.chars().all(|char| char.is_ascii_digit())
    });
    let current = match kind {
        "pane" => value == context.pane,
        "surface" => value == context.surface,
        "workspace" => value == context.workspace,
        _ => false,
    };
    let known = if kind == "workspace" {
        context.workspaces.contains(value)
    } else {
        context.tree.contains(value)
    };
    if !valid || (!current && !known) {
        return Err(io::Error::other(format!(
            "unknown {kind} reference: {value}"
        )));
    }
    Ok(())
}

fn destructive(request: &str) -> io::Result<()> {
    let explicit = request
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| {
            matches!(
                word.to_ascii_lowercase().as_str(),
                "clear" | "close" | "delete" | "kill" | "quit" | "remove"
            )
        });
    if !explicit {
        return Err(io::Error::other(
            "a destructive CMUX action was not explicitly requested",
        ));
    }
    Ok(())
}

fn text(value: &str, limit: usize) -> io::Result<()> {
    if value.is_empty()
        || value.len() > limit
        || value.starts_with('-')
        || value.chars().any(char::is_control)
    {
        return Err(io::Error::other("invalid CMUX action text"));
    }
    Ok(())
}
