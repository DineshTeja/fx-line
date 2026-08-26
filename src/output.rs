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
    let payload: Value = serde_json::from_str(output.trim())
        .map_err(|_| InvalidResponse("model returned invalid JSON"))?;
    let object = payload
        .as_object()
        .ok_or(InvalidResponse("model returned invalid JSON"))?;
    if object.len() != 1 {
        return Err(InvalidResponse("model returned unexpected fields"));
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .ok_or(InvalidResponse("model returned no command"))?
        .trim();

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
    }

    #[test]
    fn rejects_invalid_output() {
        for raw in [
            "not json".into(),
            response(r#"{"command":"pwd"}"#, 1),
            response("pwd", 0),
            response(r#"{"command":""}"#, 0),
            response(r#"{"command":"pwd","note":"extra"}"#, 0),
            response(r#"{"command":"pwd\nls"}"#, 0),
            response(r#"{"command":"pwd\u0000ls"}"#, 0),
            response(&json!({ "command": "x".repeat(8_193) }).to_string(), 0),
        ] {
            assert!(parse(&raw).is_err());
        }
    }
}
