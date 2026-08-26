use super::Context;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Down,
    Left,
    Right,
    Up,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Up => "up",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeDirection {
    Down,
    Left,
    Right,
    Up,
}

impl ResizeDirection {
    fn flag(self) -> &'static str {
        match self {
            Self::Down => "-D",
            Self::Left => "-L",
            Self::Right => "-R",
            Self::Up => "-U",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarAction {
    Focus,
    Hide,
    Show,
    Toggle,
}

impl SidebarAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Hide => "hide",
            Self::Show => "show",
            Self::Toggle => "toggle",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    CloseSurface {
        surface: Option<String>,
    },
    CloseWorkspace {
        workspace: Option<String>,
    },
    Flash,
    FocusPane {
        pane: String,
    },
    MoveSurface {
        pane: String,
        surface: String,
    },
    NewTerminal {
        direction: Direction,
    },
    NewWorkspace {
        cwd: Option<PathBuf>,
        name: Option<String>,
    },
    Open {
        target: String,
    },
    OpenBrowser {
        direction: Option<Direction>,
        url: String,
    },
    RenameTab {
        name: String,
        surface: Option<String>,
    },
    RenameWorkspace {
        name: String,
        workspace: Option<String>,
    },
    ResizePane {
        amount: u16,
        direction: ResizeDirection,
        pane: String,
    },
    SelectWorkspace {
        workspace: String,
    },
    Sidebar {
        action: SidebarAction,
    },
    SplitSurface {
        direction: Direction,
        surface: String,
    },
    SwapPanes {
        pane: String,
        target: String,
    },
}

macro_rules! argv {
    ($($value:expr),+ $(,)?) => {
        vec![$($value.to_string()),+]
    };
}

pub(super) fn arguments(context: &Context, action: &Action) -> Vec<String> {
    match action {
        Action::CloseSurface { surface } => scoped(
            context,
            argv!(
                "close-surface",
                "--surface",
                surface.as_deref().unwrap_or(&context.surface)
            ),
        ),
        Action::CloseWorkspace { workspace } => windowed(
            context,
            argv!(
                "close-workspace",
                "--workspace",
                workspace.as_deref().unwrap_or(&context.workspace)
            ),
        ),
        Action::Flash => scoped(context, argv!("trigger-flash")),
        Action::FocusPane { pane } => scoped(context, argv!("focus-pane", "--pane", pane)),
        Action::MoveSurface { pane, surface } => scoped(
            context,
            argv!("move-surface", "--surface", surface, "--pane", pane),
        ),
        Action::NewTerminal { direction } => scoped(
            context,
            argv!(
                "new-pane",
                "--type",
                "terminal",
                "--direction",
                direction.as_str(),
                "--focus",
                "true"
            ),
        ),
        Action::NewWorkspace { cwd, name } => {
            let mut args = argv!("new-workspace", "--focus", "true");
            if let Some(name) = name {
                args.extend(argv!("--name", name));
            }
            if let Some(cwd) = cwd {
                args.extend(argv!("--cwd", cwd.display()));
            }
            windowed(context, args)
        }
        Action::Open { target } => scoped(context, argv!("open", target, "--focus", "true")),
        Action::OpenBrowser { direction, url } => scoped(
            context,
            argv!(
                "new-pane",
                "--type",
                "browser",
                "--direction",
                direction.unwrap_or(Direction::Right).as_str(),
                "--url",
                url,
                "--focus",
                "false"
            ),
        ),
        Action::RenameTab { name, surface } => scoped(
            context,
            argv!(
                "rename-tab",
                "--surface",
                surface.as_deref().unwrap_or(&context.surface),
                name
            ),
        ),
        Action::RenameWorkspace { name, workspace } => windowed(
            context,
            argv!(
                "rename-workspace",
                "--workspace",
                workspace.as_deref().unwrap_or(&context.workspace),
                name
            ),
        ),
        Action::ResizePane {
            amount,
            direction,
            pane,
        } => scoped(
            context,
            argv!(
                "resize-pane",
                "--pane",
                pane,
                direction.flag(),
                "--amount",
                amount
            ),
        ),
        Action::SelectWorkspace { workspace } => {
            windowed(context, argv!("select-workspace", "--workspace", workspace))
        }
        Action::Sidebar { action } => scoped(context, argv!("right-sidebar", action.as_str())),
        Action::SplitSurface { direction, surface } => scoped(
            context,
            argv!(
                "split-off",
                "--surface",
                surface,
                direction.as_str(),
                "--focus",
                "true"
            ),
        ),
        Action::SwapPanes { pane, target } => scoped(
            context,
            argv!("swap-pane", "--pane", pane, "--target-pane", target),
        ),
    }
}

fn scoped(context: &Context, mut args: Vec<String>) -> Vec<String> {
    args.extend([
        "--workspace".into(),
        context.workspace.clone(),
        "--window".into(),
        context.window.clone(),
    ]);
    args
}

fn windowed(context: &Context, mut args: Vec<String>) -> Vec<String> {
    args.extend(["--window".into(), context.window.clone()]);
    args
}

#[cfg(test)]
mod tests {
    use super::{Action, Direction, arguments};
    use crate::cmux::{Context, validation::validate};
    use std::path::PathBuf;

    fn context() -> Context {
        Context {
            cwd: Some(PathBuf::from("/tmp")),
            pane: "pane:1".into(),
            surface: "surface:1".into(),
            surface_type: "terminal".into(),
            tree: "pane:1 surface:1 pane:2 surface:2 workspace:1 workspace:2".into(),
            window: "window:1".into(),
            workspace: "workspace:1".into(),
            workspaces: "workspace:1 pane:1 workspace:2 pane:17".into(),
            socket: "/tmp/cmux.sock".into(),
        }
    }

    #[test]
    fn compiles_browser_action_to_exact_cmux_argv() {
        let context = context();
        let action = Action::OpenBrowser {
            direction: Some(Direction::Right),
            url: "https://github.com".into(),
        };
        assert!(validate(&context, "open github", &action).is_ok());
        assert_eq!(
            arguments(&context, &action),
            [
                "new-pane",
                "--type",
                "browser",
                "--direction",
                "right",
                "--url",
                "https://github.com",
                "--focus",
                "false",
                "--workspace",
                "workspace:1",
                "--window",
                "window:1",
            ]
        );
    }

    #[test]
    fn validates_refs_and_destructive_intent() {
        let context = context();
        assert!(
            validate(
                &context,
                "swap panes",
                &Action::SwapPanes {
                    pane: "pane:1".into(),
                    target: "pane:2".into(),
                },
            )
            .is_ok()
        );
        assert!(
            validate(
                &context,
                "organize panes",
                &Action::CloseSurface { surface: None },
            )
            .is_err()
        );
        assert!(
            validate(
                &context,
                "swap panes",
                &Action::SwapPanes {
                    pane: "pane:1".into(),
                    target: "pane:17".into(),
                },
            )
            .is_err()
        );
        assert!(
            validate(
                &context,
                "focus pane",
                &Action::FocusPane {
                    pane: "pane:99".into(),
                },
            )
            .is_err()
        );
    }
}
