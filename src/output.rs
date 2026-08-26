use serde_json::Value;
use std::{error::Error, fmt};

const MAX_COMMAND_BYTES: usize = 8_192;

#[derive(Debug)]
pub struct InvalidResponse(&'static str);

impl fmt::Display for InvalidResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for InvalidResponse {}

pub fn parse(raw: &str) -> Result<String, InvalidResponse> {
    let envelope: Value =
        serde_json::from_str(raw).map_err(|_| InvalidResponse("fx returned invalid JSON"))?;

    if envelope.get("exit_code").and_then(Value::as_i64) != Some(0) {
        return Err(InvalidResponse("fx request failed"));
    }

    let output = envelope
        .get("output")
        .and_then(Value::as_str)
        .ok_or(InvalidResponse("fx returned no command"))?;
    let command = one_line(&model_command(unfence(output))?);
    let command = command.trim();

    if command.is_empty() {
        return Err(InvalidResponse("fx returned a blank command"));
    }
    if command.lines().count() != 1 || command.chars().any(char::is_control) {
        return Err(InvalidResponse("fx returned more than one line"));
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(InvalidResponse("fx returned an oversized command"));
    }

    Ok(command.into())
}

fn model_command(output: &str) -> Result<String, InvalidResponse> {
    let Ok(payload) = serde_json::from_str::<Value>(output) else {
        if output.starts_with('{')
            || output.starts_with('[')
            || output.contains("</")
            || output.contains("```")
        {
            return Err(InvalidResponse("model returned invalid output"));
        }
        return Ok(output.into());
    };
    let object = payload
        .as_object()
        .ok_or(InvalidResponse("model returned invalid JSON"))?;
    if object.len() != 1 {
        return Err(InvalidResponse("model returned unexpected fields"));
    }

    object
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(InvalidResponse("model returned no command"))
}

fn one_line(command: &str) -> String {
    command
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" && ")
}

fn unfence(output: &str) -> &str {
    let output = output.trim();
    let Some((header, body)) = output.split_once('\n') else {
        return output;
    };
    if !header.starts_with("```") {
        return output;
    }

    body.strip_suffix("```").map(str::trim).unwrap_or(output)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use serde_json::json;

    fn response(output: &str, exit_code: i32) -> String {
        json!({ "output": output, "exit_code": exit_code }).to_string()
    }

    #[test]
    fn accepts_one_command() {
        assert_eq!(
            parse(&response(r#"{"command":"git status --short"}"#, 0)).unwrap(),
            "git status --short"
        );
        assert_eq!(
            parse(&response("```bash\n{\"command\":\"pwd\"}\n```", 0)).unwrap(),
            "pwd"
        );
        assert_eq!(parse(&response("pwd", 0)).unwrap(), "pwd");
        assert_eq!(
            parse(&response(
                &json!({ "command": "find . \\\n | sort" }).to_string(),
                0,
            ))
            .unwrap(),
            "find .   | sort"
        );
        assert_eq!(
            parse(&response(r#"{"command":"cd /tmp\npwd"}"#, 0)).unwrap(),
            "cd /tmp && pwd"
        );
    }

    #[test]
    fn rejects_invalid_output() {
        for raw in [
            "not json".into(),
            response(r#"{"command":"pwd"}"#, 1),
            response("result:\n```json\n{\"command\":\"pwd\"}\n```", 0),
            response(r#"{"command":""}"#, 0),
            response(r#"{"command":"pwd","note":"extra"}"#, 0),
            response(r#"{"command":"pwd\u0000ls"}"#, 0),
            response("pwd</arg_value></tool_call>", 0),
            response(&json!({ "command": "x".repeat(8_193) }).to_string(), 0),
        ] {
            assert!(parse(&raw).is_err());
        }
    }
}
