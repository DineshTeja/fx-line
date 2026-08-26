mod actions;
mod validation;

pub use actions::Action;
use serde::{Deserialize, Serialize};
use std::{
    env,
    error::Error,
    ffi::OsString,
    io,
    path::PathBuf,
    process::{Command, Output},
};

const STATUS_KEY: &str = "voice-agent";

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Debug, Serialize)]
pub struct Context {
    pub cwd: Option<PathBuf>,
    pub pane: String,
    pub surface: String,
    pub surface_type: String,
    pub tree: String,
    pub window: String,
    pub workspace: String,
    pub workspaces: String,
    #[serde(skip)]
    socket: String,
}

pub struct Cmux {
    binary: OsString,
}

impl Default for Cmux {
    fn default() -> Self {
        Self::new()
    }
}

impl Cmux {
    pub fn new() -> Self {
        let binary = env::var_os("FX_AGENT_CMUX")
            .or_else(|| env::var_os("CMUX_BUNDLED_CLI_PATH"))
            .unwrap_or_else(|| "/Applications/cmux.app/Contents/Resources/bin/cmux".into());
        Self { binary }
    }

    pub fn context(&self) -> Result<Context> {
        let identity: Identity =
            serde_json::from_slice(&self.output(["identify", "--no-caller", "--json"])?.stdout)?;
        let focused = identity
            .focused
            .ok_or_else(|| io::Error::other("cmux has no focused surface"))?;
        let tree = bounded(
            self.text(["tree", "--workspace", &focused.workspace_ref])?,
            8_000,
        );
        let workspaces = bounded(
            self.text(["tree", "--window", &focused.window_ref])?,
            12_000,
        );
        let cwd = (focused.surface_type == "terminal")
            .then(|| self.focused_cwd(&focused.surface_ref))
            .flatten();

        Ok(Context {
            cwd,
            pane: focused.pane_ref,
            surface: focused.surface_ref,
            surface_type: focused.surface_type,
            tree,
            window: focused.window_ref,
            workspace: focused.workspace_ref,
            workspaces,
            socket: identity.socket_path,
        })
    }

    pub fn clear_agent_statuses(&self) -> Result<()> {
        let tree: Tree = serde_json::from_slice(&self.output(["tree", "--json"])?.stdout)?;
        for window in tree.windows {
            for workspace in window.workspaces {
                self.output([
                    "clear-status",
                    STATUS_KEY,
                    "--workspace",
                    &workspace.reference,
                    "--window",
                    &window.reference,
                ])?;
            }
        }
        Ok(())
    }

    pub fn notify(&self, context: &Context, body: &str) -> Result<()> {
        self.context_output(
            context,
            [
                "notify",
                "--title",
                "Voice agent",
                "--body",
                body,
                "--workspace",
                &context.workspace,
                "--window",
                &context.window,
            ],
        )?;
        Ok(())
    }

    pub fn validate(&self, context: &Context, request: &str, action: &Action) -> Result<()> {
        Ok(validation::validate(context, request, action)?)
    }

    pub fn execute(&self, context: &Context, request: &str, action: &Action) -> Result<()> {
        self.validate(context, request, action)?;
        self.context_output(context, actions::arguments(context, action))?;
        Ok(())
    }

    fn focused_cwd(&self, surface: &str) -> Option<PathBuf> {
        let top = self.text(["top", "--all", "--processes", "--flat"]).ok()?;
        let shell_pids = top.lines().filter_map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
            (fields.len() >= 7
                && fields[3] == "process"
                && fields[5] == surface
                && matches!(fields[6], "zsh" | "bash" | "fish" | "sh" | "nu"))
            .then_some(fields[4])
        });

        for pid in shell_pids {
            let output = Command::new("/usr/sbin/lsof")
                .args(["-a", "-p", pid, "-d", "cwd", "-Fn"])
                .output()
                .ok()?;
            if let Some(path) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .find_map(|line| line.strip_prefix('n'))
            {
                return Some(PathBuf::from(path));
            }
        }
        None
    }

    fn text<const N: usize>(&self, args: [&str; N]) -> Result<String> {
        Ok(String::from_utf8(self.output(args)?.stdout)?)
    }

    fn output<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        checked(Command::new(&self.binary).args(args).output()?)
    }

    fn context_output<I, S>(&self, context: &Context, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        checked(
            Command::new(&self.binary)
                .args(args)
                .env("CMUX_SOCKET_PATH", &context.socket)
                .env("CMUX_WORKSPACE_ID", &context.workspace)
                .env("CMUX_SURFACE_ID", &context.surface)
                .output()?,
        )
    }
}

fn bounded(mut value: String, bytes: usize) -> String {
    if value.len() <= bytes {
        return value;
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push('…');
    value
}

fn checked(output: Output) -> Result<Output> {
    if output.status.success() {
        return Ok(output);
    }
    let error = String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("cmux command failed")
        .to_owned();
    Err(io::Error::other(error).into())
}

#[derive(Deserialize)]
struct Identity {
    focused: Option<Focused>,
    socket_path: String,
}

#[derive(Deserialize)]
struct Focused {
    pane_ref: String,
    surface_ref: String,
    surface_type: String,
    window_ref: String,
    workspace_ref: String,
}

#[derive(Deserialize)]
struct Tree {
    windows: Vec<TreeWindow>,
}

#[derive(Deserialize)]
struct TreeWindow {
    #[serde(rename = "ref")]
    reference: String,
    workspaces: Vec<TreeWorkspace>,
}

#[derive(Deserialize)]
struct TreeWorkspace {
    #[serde(rename = "ref")]
    reference: String,
}
