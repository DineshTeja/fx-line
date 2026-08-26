use serde_json::{Map, Value};
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

const FLOW_SHORTCUT: &str = "59+63";
const FLOW_PTT: &str = "ptt";
const FLOW_COMMAND_MODE: &str = "lens";
const FLOW_BINARY: &str = "/Applications/Wispr Flow.app/Contents/MacOS/Wispr Flow";
const FLOW_GRACEFUL_STOP: Duration = Duration::from_secs(1);
const FLOW_STOP_TIMEOUT: Duration = Duration::from_secs(5);
const OLD_EXTENSION: &str = "fx-agent";

pub fn install(home: &Path) -> io::Result<()> {
    remove_old_extension(home)?;
    set_shortcut(home, FLOW_PTT)
}

pub fn uninstall(home: &Path) -> io::Result<()> {
    remove_old_extension(home)?;
    set_shortcut(home, FLOW_COMMAND_MODE)
}

pub fn stop_if_running() -> io::Result<bool> {
    let Some(pid) = process_id()? else {
        return Ok(false);
    };
    checked(
        Command::new("/usr/bin/osascript")
            .args(["-e", "tell application \"Wispr Flow\" to quit"])
            .output()?,
    )?;
    let started = Instant::now();
    while started.elapsed() < FLOW_GRACEFUL_STOP {
        if process_id()?.is_none() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("/bin/kill").arg(pid.to_string()).output();
    while started.elapsed() < FLOW_STOP_TIMEOUT {
        if process_id()?.is_none() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other("Wispr Flow did not stop"))
}

pub fn start() -> io::Result<()> {
    checked(
        Command::new("/usr/bin/open")
            .args(["-j", "-a", "/Applications/Wispr Flow.app"])
            .output()?,
    )?;
    Ok(())
}

fn set_shortcut(home: &Path, action: &str) -> io::Result<()> {
    let config = flow_root(home).join("config.json");
    update_json(&config, Value::Null, |value| {
        object_at(value, &["prefs", "user", "shortcuts"])?
            .insert(FLOW_SHORTCUT.into(), Value::String(action.into()));
        Ok(())
    })
}

fn remove_old_extension(home: &Path) -> io::Result<()> {
    let root = bridge_root(home);
    let flow_extensions = flow_root(home).join("extensions");
    let custom_paths = flow_extensions.join("custom-paths.json");
    let enabled_state = flow_extensions.join("extensions-state.json");

    if custom_paths.exists() {
        update_json(&custom_paths, Value::Array(Vec::new()), |value| {
            let paths = value.as_array_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Wispr custom paths")
            })?;
            let root = root.to_string_lossy();
            paths.retain(|path| path.as_str() != Some(root.as_ref()));
            Ok(())
        })?;
    }
    if enabled_state.exists() {
        update_json(&enabled_state, Value::Object(Map::new()), |value| {
            object(value, "invalid Wispr extension state")?.remove(OLD_EXTENSION);
            Ok(())
        })?;
    }
    remove_dir(&root)?;
    remove_file(&state_root(home).join("agent.sock"))
}

fn process_id() -> io::Result<Option<u32>> {
    let output = checked(
        Command::new("/bin/ps")
            .args(["-axo", "pid=,command="])
            .output()?,
    )?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        let Some((pid, command)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if command.trim() == FLOW_BINARY {
            return pid.parse().map(Some).map_err(io::Error::other);
        }
    }
    Ok(None)
}

fn update_json(
    path: &Path,
    default: Value,
    update: impl FnOnce(&mut Value) -> io::Result<()>,
) -> io::Result<()> {
    let mut value = match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents).map_err(io::Error::other)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => default,
        Err(error) => return Err(error),
    };
    update(&mut value)?;
    let mut contents = serde_json::to_vec_pretty(&value).map_err(io::Error::other)?;
    contents.push(b'\n');
    replace(path, &contents)
}

fn object<'a>(
    value: &'a mut Value,
    message: &'static str,
) -> io::Result<&'a mut Map<String, Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, message))
}

fn object_at<'a>(value: &'a mut Value, path: &[&str]) -> io::Result<&'a mut Map<String, Value>> {
    let mut value = value;
    for key in path {
        value = value.get_mut(*key).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Wispr config is missing {}", path.join(".")),
            )
        })?;
    }
    object(value, "Wispr shortcuts are not an object")
}

fn replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("destination has no parent"))?;
    fs::create_dir_all(parent)?;
    let pending = path.with_extension("new");
    fs::write(&pending, contents)?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&pending, metadata.permissions())?;
    }
    fs::rename(pending, path)
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn state_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/fx-line")
}

fn bridge_root(home: &Path) -> PathBuf {
    state_root(home).join("wispr")
}

fn flow_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Wispr Flow")
}

fn checked(output: Output) -> io::Result<Output> {
    if output.status.success() {
        return Ok(output);
    }
    let message = String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("command failed")
        .to_owned();
    Err(io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use super::{FLOW_PTT, FLOW_SHORTCUT, object_at};
    use serde_json::json;

    #[test]
    fn routes_only_function_control_to_push_to_talk() {
        let mut config = json!({"prefs":{"user":{"shortcuts":{"63":"ptt"}}}});
        object_at(&mut config, &["prefs", "user", "shortcuts"])
            .unwrap()
            .insert(FLOW_SHORTCUT.into(), FLOW_PTT.into());
        assert_eq!(config["prefs"]["user"]["shortcuts"]["63"], "ptt");
        assert_eq!(config["prefs"]["user"]["shortcuts"]["59+63"], "ptt",);
    }
}
