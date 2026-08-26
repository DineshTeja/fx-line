use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

const LABEL: &str = "com.dineshteja.fx-agent";

pub fn install(binary: &Path) -> io::Result<()> {
    let home = home()?;
    let agents = home.join("Library/LaunchAgents");
    let state = home.join("Library/Application Support/fx-line");
    fs::create_dir_all(&agents)?;
    fs::create_dir_all(&state)?;

    let plist = agents.join(format!("{LABEL}.plist"));
    let contents = plist_contents(binary, &home, &state);
    let pending = plist.with_extension("plist.new");
    fs::write(&pending, contents)?;
    fs::rename(pending, &plist)?;

    let service = format!("{}/{}", domain()?, LABEL);
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output();
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
    thread::sleep(Duration::from_millis(300));
    if is_running()? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "grant Accessibility to ~/.local/bin/fx-agent, then run `fx-agent install` again",
        ))
    }
}

pub fn uninstall() -> io::Result<()> {
    let plist = home()?.join(format!("Library/LaunchAgents/{LABEL}.plist"));
    let service = format!("{}/{}", domain()?, LABEL);
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output();
    match fs::remove_file(plist) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn is_running() -> io::Result<bool> {
    let service = format!("{}/{}", domain()?, LABEL);
    let output = Command::new("/bin/launchctl")
        .args(["print", &service])
        .output()?;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout).contains("\n\tstate = running"))
}

fn plist_contents(binary: &Path, home: &Path, state: &Path) -> String {
    let binary = escape(&binary.to_string_lossy());
    let path = escape(&format!(
        "{}/.local/bin:{}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        home.display(),
        home.display()
    ));
    let home = escape(&home.to_string_lossy());
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
    <string>{binary}</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
    <key>PATH</key>
    <string>{path}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
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

fn checked(output: std::process::Output) -> io::Result<std::process::Output> {
    if output.status.success() {
        return Ok(output);
    }
    let message = String::from_utf8_lossy(&output.stderr)
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("launchctl failed")
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
    use super::{escape, plist_contents};
    use std::path::Path;

    #[test]
    fn escapes_plist_values() {
        assert_eq!(escape("a&<\"'"), "a&amp;&lt;&quot;&apos;");
    }

    #[test]
    fn plist_has_one_small_background_process() {
        let plist = plist_contents(
            Path::new("/tmp/fx & agent"),
            Path::new("/Users/test"),
            Path::new("/tmp/state"),
        );
        assert!(plist.contains("/tmp/fx &amp; agent"));
        assert!(plist.contains("<string>Background</string>"));
        assert!(plist.contains("<string>run</string>"));
        assert!(plist.contains("<key>SuccessfulExit</key>"));
    }
}
