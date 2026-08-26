use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_AGENT_MODEL: &str = "zai/glm-5.2-fast";
const DEFAULT_LINE_MODEL: &str = "zai/glm-4.7-flash";
const DEFAULT_PLAN_MODEL: &str = "zai/glm-4.7-flash";
const MAX_ATTEMPTS: usize = 2;
const LINE_TIMEOUT: Duration = Duration::from_secs(5);
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

pub fn generate(request: &str, cwd: &str, current_line: &str) -> Result<String> {
    let prompt = line_prompt(request, cwd, current_line);
    let mut last_error = None;

    for _ in 0..MAX_ATTEMPTS {
        match complete(&prompt).and_then(|output| Ok(crate::output::command(&output)?)) {
            Ok(command) => return Ok(command),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.expect("at least one fx attempt"))
}

pub fn complete(prompt: &str) -> Result<String> {
    request(
        prompt,
        None,
        env::var("FX_LINE_MODEL").unwrap_or_else(|_| DEFAULT_LINE_MODEL.into()),
        "ask",
        0,
        LINE_TIMEOUT,
    )
}

pub fn plan(prompt: &str) -> Result<String> {
    request(
        prompt,
        None,
        env::var("FX_AGENT_PLAN_MODEL")
            .or_else(|_| env::var("FX_LINE_MODEL"))
            .unwrap_or_else(|_| DEFAULT_PLAN_MODEL.into()),
        "ask",
        0,
        LINE_TIMEOUT,
    )
}

pub fn run_agent(prompt: &str, cwd: &Path) -> Result<String> {
    request(prompt, Some(cwd), agent_model(), "auto", 8, AGENT_TIMEOUT)
}

fn agent_model() -> String {
    env::var("FX_AGENT_MODEL")
        .or_else(|_| env::var("FX_LINE_MODEL"))
        .unwrap_or_else(|_| DEFAULT_AGENT_MODEL.into())
}

fn request(
    prompt: &str,
    cwd: Option<&Path>,
    model: String,
    permission: &str,
    max_steps: u8,
    timeout: Duration,
) -> Result<String> {
    let workspace = cwd.is_none().then(Workspace::new).transpose()?;
    let cwd = cwd.unwrap_or_else(|| workspace.as_ref().expect("temporary workspace").path());
    let binary = binary();

    let mut child = Command::new(binary)
        .args([
            "--no-additional-dirs",
            "ask",
            "--no-save",
            "--json",
            "--no-color",
        ])
        .current_dir(cwd)
        .env("PATH", runtime_path())
        .env("FX_MODEL", model)
        .env("FX_PERMISSION_MODE", permission)
        .env("FX_MAX_AGENT_STEPS", max_steps.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = read_in_background(
        child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("fx stdout is unavailable"))?,
    )?;
    let stderr = read_in_background(
        child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("fx stderr is unavailable"))?,
    )?;

    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("fx stdin is unavailable"))?
        .write_all(prompt.as_bytes());
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }

    let (status, timed_out) = wait(&mut child, timeout)?;
    let stdout = String::from_utf8(join(stdout)?)?;
    let stderr = String::from_utf8(join(stderr)?)?;

    if timed_out {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("fx timed out after {} seconds", timeout.as_secs()),
        )
        .into());
    }
    if !status.success() {
        let detail = stderr.lines().find(|line| !line.trim().is_empty());
        return Err(io::Error::other(detail.unwrap_or("fx failed")).into());
    }

    Ok(crate::output::envelope(&stdout)?)
}

fn runtime_path() -> OsString {
    let mut paths = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin"));
        paths.push(home.join(".cargo/bin"));
    }
    paths.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    if let Some(inherited) = env::var_os("PATH") {
        paths.extend(env::split_paths(&inherited));
    }

    env::join_paths(paths).unwrap_or_else(|_| env::var_os("PATH").unwrap_or_default())
}

fn binary() -> OsString {
    if let Some(binary) = env::var_os("FX_LINE_FX") {
        return binary;
    }

    let mut candidates = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".local/bin/fx"));
        candidates.push(home.join(".cargo/bin/fx"));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/fx"),
        PathBuf::from("/usr/local/bin/fx"),
    ]);

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| "fx".into())
}

fn line_prompt(request: &str, cwd: &str, current_line: &str) -> String {
    let directory = crate::context::directory(Path::new(cwd));
    let context = serde_json::json!({
        "request": request,
        "cwd": cwd,
        "current_line": current_line,
        "directory": directory,
    });

    format!(
        "Return one macOS zsh command without using tools. The shell is already at cwd. Reply only with JSON: {{\"command\":\"...\"}}.\n{context}"
    )
}

fn read_in_background<R>(mut reader: R) -> io::Result<JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("fx-output".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
}

fn join(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| io::Error::other("fx output reader panicked"))?
}

fn wait(child: &mut Child, timeout: Duration) -> io::Result<(ExitStatus, bool)> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            return child.wait().map(|status| (status, true));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct Workspace(PathBuf);

impl Workspace {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!("fx-line-{}-{nonce}", process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::line_prompt;

    #[test]
    fn prompt_encodes_directory_context_as_data() {
        let prompt = line_prompt("find \"notes\"", "/missing/a b", "git ");
        let (_, context) = prompt.split_once('\n').unwrap();
        let context: serde_json::Value = serde_json::from_str(context).unwrap();

        assert_eq!(context["request"], "find \"notes\"");
        assert_eq!(context["cwd"], "/missing/a b");
        assert_eq!(context["current_line"], "git ");
        assert_eq!(context["directory"]["entries"], serde_json::json!([]));
    }
}
