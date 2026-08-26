use serde::{Deserialize, de::DeserializeOwned};
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

pub fn envelope(raw: &str) -> Result<String, InvalidResponse> {
    let envelope: Value =
        serde_json::from_str(raw).map_err(|_| InvalidResponse("fx returned invalid JSON"))?;
    if envelope.get("exit_code").and_then(Value::as_i64) != Some(0) {
        return Err(InvalidResponse("fx request failed"));
    }

    envelope
        .get("output")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(InvalidResponse("fx returned no output"))
}

pub fn command(output: &str) -> Result<String, InvalidResponse> {
    let command = one_line(&json::<Command>(output)?.command);
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

pub fn json<T: DeserializeOwned>(output: &str) -> Result<T, InvalidResponse> {
    let output = unfence(output);
    if let Ok(value) = serde_json::from_str(output) {
        return Ok(value);
    }

    for (index, _) in output.match_indices('{') {
        let mut values = serde_json::Deserializer::from_str(&output[index..]).into_iter::<Value>();
        if let Some(Ok(value)) = values.next()
            && let Ok(value) = serde_json::from_value(value)
        {
            return Ok(value);
        }
    }

    Err(InvalidResponse("model returned invalid output"))
}

#[derive(Deserialize)]
struct Command {
    command: String,
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
    use super::{command, envelope};
    use serde_json::json;

    fn response(output: &str, exit_code: i32) -> String {
        json!({ "output": output, "exit_code": exit_code }).to_string()
    }

    #[test]
    fn reads_fx_envelope() {
        assert_eq!(envelope(&response("hello", 0)).unwrap(), "hello");
        assert!(envelope("not json").is_err());
        assert!(envelope(&response("hello", 1)).is_err());
    }

    #[test]
    fn accepts_one_command() {
        for (raw, expected) in [
            (r#"{"command":"git status --short"}"#, "git status --short"),
            ("```bash\n{\"command\":\"pwd\"}\n```", "pwd"),
            (
                "Here:\n{\"command\":\"pwd\",\"description\":\"working directory\"}",
                "pwd",
            ),
            (r#"{"command":"cd /tmp\npwd"}"#, "cd /tmp && pwd"),
        ] {
            assert_eq!(command(raw).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_invalid_command_output() {
        for raw in [
            "pwd".into(),
            r#"{"command":""}"#.into(),
            r#"{"command":"pwd\u0000ls"}"#.into(),
            "pwd</arg_value></tool_call>".into(),
            json!({ "command": "x".repeat(8_193) }).to_string(),
        ] {
            assert!(command(&raw).is_err());
        }
    }
}
