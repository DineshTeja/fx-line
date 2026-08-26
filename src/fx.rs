use std::{
    env,
    error::Error,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{self, Child, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DEFAULT_MODEL: &str = "zai/glm-4.7-flash";
const TIMEOUT: Duration = Duration::from_secs(5);

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

pub fn generate(request: &str, cwd: &str, current_line: &str) -> Result<String> {
    let workspace = Workspace::new()?;
    let prompt = prompt(request, cwd, current_line)?;
    let model = env::var("FX_LINE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let binary = env::var_os("FX_LINE_FX").unwrap_or_else(|| "fx".into());

    let mut child = Command::new(binary)
        .args([
            "--no-additional-dirs",
            "ask",
            "--no-save",
            "--json",
            "--no-color",
        ])
        .current_dir(workspace.path())
        .env("FX_MODEL", model)
        .env("FX_PERMISSION_MODE", "ask")
        .env("FX_MAX_AGENT_STEPS", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = read_in_background(
        child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("fx stdout is unavailable"))?,
    );
    let stderr = read_in_background(
        child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("fx stderr is unavailable"))?,
    );

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

    let (status, timed_out) = wait(&mut child)?;
    let stdout = String::from_utf8(join(stdout)?)?;
    let stderr = String::from_utf8(join(stderr)?)?;

    if timed_out {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("fx timed out after {} seconds", TIMEOUT.as_secs()),
        )
        .into());
    }
    if !status.success() {
        let detail = stderr.lines().find(|line| !line.trim().is_empty());
        return Err(io::Error::other(detail.unwrap_or("fx failed")).into());
    }

    Ok(crate::output::parse(&stdout)?)
}

fn prompt(request: &str, cwd: &str, current_line: &str) -> Result<String> {
    Ok(format!(
        "Return one macOS zsh command without using tools. The shell is already in cwd.\nReply only as JSON: {{\"command\":\"...\"}}\nrequest={}\ncwd={}\ninput={}",
        serde_json::to_string(request)?,
        serde_json::to_string(cwd)?,
        serde_json::to_string(current_line)?,
    ))
}

fn read_in_background<R>(mut reader: R) -> JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
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

fn wait(child: &mut Child) -> io::Result<(ExitStatus, bool)> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, false));
        }
        if started.elapsed() >= TIMEOUT {
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
    use super::prompt;

    #[test]
    fn prompt_encodes_context_as_data() {
        let prompt = prompt("find \"notes\"", "/tmp/a b", "git ").unwrap();

        assert_eq!(
            prompt,
            "Return one macOS zsh command without using tools. The shell is already in cwd.\nReply only as JSON: {\"command\":\"...\"}\nrequest=\"find \\\"notes\\\"\"\ncwd=\"/tmp/a b\"\ninput=\"git \""
        );
    }
}
