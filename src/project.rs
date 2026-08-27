use serde::Serialize;
use std::{fs, path::Path};

const MAX_ENTRIES: usize = 48;
const MAX_SCANNED_ENTRIES: usize = 512;

#[derive(Debug, Serialize)]
pub(crate) struct DirectoryContext {
    entries: Vec<String>,
    git_branch: Option<String>,
    git_root: Option<String>,
}

pub(crate) fn directory(path: &Path) -> DirectoryContext {
    let mut entries = fs::read_dir(path)
        .into_iter()
        .flatten()
        .take(MAX_SCANNED_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if name == ".DS_Store" || name.chars().any(char::is_control) {
                return None;
            }
            if entry.file_type().ok()?.is_dir() {
                name.push('/');
            }
            Some(name)
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    entries.truncate(MAX_ENTRIES);

    let git_root = path.ancestors().find(|parent| parent.join(".git").exists());
    let git_branch = git_root.and_then(branch);

    DirectoryContext {
        entries,
        git_branch,
        git_root: git_root.map(|root| root.to_string_lossy().into_owned()),
    }
}

fn branch(root: &Path) -> Option<String> {
    let dot_git = root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let pointer = fs::read_to_string(dot_git).ok()?;
        let path = pointer.trim().strip_prefix("gitdir: ")?;
        root.join(path)
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    Some(
        head.trim()
            .strip_prefix("ref: refs/heads/")
            .unwrap_or(head.trim())
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::directory;
    use std::{fs, process, time::SystemTime};

    #[test]
    fn captures_bounded_project_context() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("fx-line-context-{}-{nonce}", process::id()));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let value = serde_json::to_value(directory(&root)).unwrap();

        assert_eq!(
            value["entries"],
            serde_json::json!([".git/", "Cargo.toml", "src/"])
        );
        assert_eq!(value["git_branch"], "main");
        assert_eq!(value["git_root"], root.to_string_lossy().as_ref());
        fs::remove_dir_all(root).unwrap();
    }
}
