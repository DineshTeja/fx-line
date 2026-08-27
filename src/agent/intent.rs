use crate::cmux::{Action, Direction};

pub(super) struct Intent {
    pub(super) action: Action,
    pub(super) message: String,
}

pub(super) fn parse(request: &str) -> Option<Intent> {
    let request = request.trim().trim_end_matches(['.', '!', '?']);
    if request.eq_ignore_ascii_case("flash") {
        return Some(Intent {
            action: Action::Flash,
            message: "Flashed CMUX".into(),
        });
    }

    browser(request)
}

fn browser(request: &str) -> Option<Intent> {
    let mut target = request.strip_prefix("open ").or_else(|| {
        request
            .get(..5)
            .filter(|prefix| prefix.eq_ignore_ascii_case("open "))
            .map(|_| &request[5..])
    })?;
    let (without_direction, direction) = direction(target);
    target = without_direction;
    for suffix in [" in the browser", " in a browser", " in browser"] {
        if let Some(value) = strip_suffix_ignore_case(target, suffix) {
            target = value;
            break;
        }
    }
    target = target.trim();
    if target
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("the "))
    {
        target = target[4..].trim();
    }

    let url = url(target)?;
    Some(Intent {
        action: Action::OpenBrowser {
            direction,
            url: url.clone(),
        },
        message: format!("Opened {target}"),
    })
}

fn direction(value: &str) -> (&str, Option<Direction>) {
    for (suffix, direction) in [
        (" on the right", Direction::Right),
        (" to the right", Direction::Right),
        (" on the left", Direction::Left),
        (" to the left", Direction::Left),
        (" at the top", Direction::Up),
        (" on top", Direction::Up),
        (" at the bottom", Direction::Down),
        (" on the bottom", Direction::Down),
    ] {
        if let Some(value) = strip_suffix_ignore_case(value, suffix) {
            return (value, Some(direction));
        }
    }
    (value, None)
}

fn url(target: &str) -> Option<String> {
    if target.starts_with("https://") || target.starts_with("http://") {
        return Some(target.into());
    }
    if target.contains(char::is_whitespace)
        || target.is_empty()
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        return None;
    }

    let host = target.to_ascii_lowercase();
    Some(if host.contains('.') {
        format!("https://{host}")
    } else {
        format!("https://{host}.com")
    })
}

fn strip_suffix_ignore_case<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    let start = value.len().checked_sub(suffix.len())?;
    value[start..]
        .eq_ignore_ascii_case(suffix)
        .then_some(&value[..start])
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::cmux::{Action, Direction};

    #[test]
    fn opens_a_spoken_site_without_a_model_call() {
        let intent = parse("Open Netflix on the right.").unwrap();
        assert!(matches!(
            intent.action,
            Action::OpenBrowser {
                direction: Some(Direction::Right),
                ref url,
            } if url == "https://netflix.com"
        ));
    }

    #[test]
    fn accepts_urls_but_not_file_requests() {
        assert!(parse("open github.com in the browser").is_some());
        assert!(parse("open https://example.com").is_some());
        assert!(parse("open the readme file").is_none());
    }
}
