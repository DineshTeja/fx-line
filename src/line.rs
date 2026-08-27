use crate::{Result, model, project};
use std::path::Path;

const MAX_ATTEMPTS: usize = 2;

pub fn generate(request: &str, cwd: &str, current_line: &str) -> Result<String> {
    let prompt = prompt(request, cwd, current_line);
    let mut last_error = None;

    for _ in 0..MAX_ATTEMPTS {
        match model::complete(&prompt).and_then(|output| Ok(model::response::command(&output)?)) {
            Ok(command) => return Ok(command),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.expect("at least one fx attempt"))
}

fn prompt(request: &str, cwd: &str, current_line: &str) -> String {
    let directory = project::directory(Path::new(cwd));
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

#[cfg(test)]
mod tests {
    use super::prompt;

    #[test]
    fn prompt_encodes_directory_context_as_data() {
        let prompt = prompt("find \"notes\"", "/missing/a b", "git ");
        let (_, context) = prompt.split_once('\n').unwrap();
        let context: serde_json::Value = serde_json::from_str(context).unwrap();

        assert_eq!(context["request"], "find \"notes\"");
        assert_eq!(context["cwd"], "/missing/a b");
        assert_eq!(context["current_line"], "git ");
        assert_eq!(context["directory"]["entries"], serde_json::json!([]));
    }
}
