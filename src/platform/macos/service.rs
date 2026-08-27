use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
    time::{Duration, Instant},
};

const LABEL: &str = "com.dineshteja.fx-agent";
const APP_NAME: &str = "fx-agent.app";
const DESIGNATED_REQUIREMENT: &str = "=designated => identifier \"com.dineshteja.fx-agent\"";
const START_TIMEOUT: Duration = Duration::from_secs(5);

pub fn install(binary: &Path) -> io::Result<()> {
    with_wispr_stopped(|| install_stopped(binary))
}

fn install_stopped(binary: &Path) -> io::Result<()> {
    let home = home()?;
    let agents = home.join("Library/LaunchAgents");
    let state = home.join("Library/Application Support/fx-line");
    let app = home.join("Applications").join(APP_NAME);
    let app_binary = app.join("Contents/MacOS/fx-agent");
    let plist = agents.join(format!("{LABEL}.plist"));
    let service = format!("{}/{}", domain()?, LABEL);

    fs::create_dir_all(&agents)?;
    fs::create_dir_all(&state)?;
    fs::create_dir_all(app_binary.parent().expect("app binary has a parent"))?;
    super::wispr::install(&home)?;
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output();
    stop(&app_binary)?;

    replace(binary, &app_binary)?;
    replace_text(&app.join("Contents/Info.plist"), &info_contents(&home))?;
    checked(
        Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--timestamp=none",
                "--requirements",
                DESIGNATED_REQUIREMENT,
            ])
            .arg(&app)
            .output()?,
    )?;
    replace_text(&plist, &plist_contents(&app, &state))?;

    checked(
        Command::new("/bin/launchctl")
            .args(["bootstrap", service_domain(&service), path(&plist)?])
            .output()?,
    )?;
    checked(
        Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &service])
            .output()?,
    )?;

    let started = Instant::now();
    while started.elapsed() < START_TIMEOUT {
        if process_id(&app_binary)?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::other("fx-agent did not start"))
}

pub fn uninstall() -> io::Result<()> {
    with_wispr_stopped(uninstall_stopped)
}

fn uninstall_stopped() -> io::Result<()> {
    let home = home()?;
    let app = home.join("Applications").join(APP_NAME);
    let app_binary = app.join("Contents/MacOS/fx-agent");
    let plist = home.join(format!("Library/LaunchAgents/{LABEL}.plist"));
    let service = format!("{}/{}", domain()?, LABEL);
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output();
    stop(&app_binary)?;
    super::wispr::uninstall(&home)?;
    remove_file(plist)?;
    match fs::remove_dir_all(app) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn with_wispr_stopped(operation: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    let restart = super::wispr::stop_if_running()?;
    let result = operation();
    let restart_result = restart.then(super::wispr::start).transpose();
    match (result, restart_result) {
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

pub fn is_running() -> io::Result<bool> {
    let binary = home()?.join(format!("Applications/{APP_NAME}/Contents/MacOS/fx-agent"));
    Ok(process_id(&binary)?.is_some())
}

fn stop(binary: &Path) -> io::Result<()> {
    let Some(pid) = process_id(binary)? else {
        return Ok(());
    };
    let _ = Command::new("/bin/kill").arg(pid.to_string()).output();
    for _ in 0..20 {
        if process_id(binary)?.is_none() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(io::Error::other("the previous fx-agent did not stop"))
}

fn process_id(binary: &Path) -> io::Result<Option<u32>> {
    let binary = binary.to_string_lossy();
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
        if command.trim() == binary {
            return pid.parse().map(Some).map_err(io::Error::other);
        }
    }
    Ok(None)
}

fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    let pending = destination.with_extension("new");
    fs::copy(source, &pending)?;
    fs::set_permissions(&pending, fs::metadata(source)?.permissions())?;
    fs::rename(pending, destination)
}

fn replace_text(destination: &Path, contents: &str) -> io::Result<()> {
    let pending = destination.with_extension("new");
    fs::write(&pending, contents)?;
    fs::rename(pending, destination)
}

fn remove_file(path: PathBuf) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn info_contents(home: &Path) -> String {
    let path = escape(&format!(
        "{}/.local/bin:{}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        home.display(),
        home.display()
    ));
    let home = escape(&home.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>fx-agent</string>
  <key>CFBundleIdentifier</key>
  <string>{LABEL}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>fx-agent</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>LSEnvironment</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
</dict>
</plist>
"#
    )
}

fn plist_contents(app: &Path, state: &Path) -> String {
    let app = escape(&app.to_string_lossy());
    let log = escape(&state.join("agent.log").to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/open</string>
    <string>-g</string>
    <string>-j</string>
    <string>{app}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

fn home() -> io::Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unavailable"))
}

fn domain() -> io::Result<String> {
    let output = checked(Command::new("/usr/bin/id").arg("-u").output()?)?;
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn service_domain(service: &str) -> &str {
    service
        .rsplit_once('/')
        .map_or(service, |(domain, _)| domain)
}

fn path(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8"))
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

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{escape, info_contents, plist_contents};
    use std::path::Path;

    #[test]
    fn escapes_plist_values() {
        assert_eq!(escape("a&<\"'"), "a&amp;&lt;&quot;&apos;");
    }

    #[test]
    fn installs_an_invisible_app() {
        let info = info_contents(Path::new("/Users/test"));
        assert!(info.contains("<key>LSUIElement</key>"));
        assert!(info.contains("com.dineshteja.fx-agent"));
    }

    #[test]
    fn launch_agent_starts_the_app_once() {
        let plist = plist_contents(
            Path::new("/Users/test/Applications/fx-agent.app"),
            Path::new("/tmp/state"),
        );
        assert!(plist.contains("<string>/usr/bin/open</string>"));
        assert!(plist.contains("<string>/Users/test/Applications/fx-agent.app</string>"));
        assert!(!plist.contains("<string>-W</string>"));
        assert!(!plist.contains("<key>KeepAlive</key>"));
    }
}
