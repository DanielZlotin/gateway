pub const TELEGRAM_MESSAGE_LIMIT: usize = 3900;

pub fn split_telegram_message(text: &str) -> Vec<String> {
    let mut rest = text.trim().to_string();
    if rest.is_empty() {
        return vec![String::new()];
    }

    let mut parts = Vec::new();
    while rest.chars().count() > TELEGRAM_MESSAGE_LIMIT {
        let chars: Vec<char> = rest.chars().collect();
        let mut split_at = TELEGRAM_MESSAGE_LIMIT;
        let floor = TELEGRAM_MESSAGE_LIMIT.saturating_sub(600);

        for index in (floor..TELEGRAM_MESSAGE_LIMIT).rev() {
            if chars[index] == '\n' {
                split_at = index + 1;
                break;
            }
        }

        parts.push(chars[..split_at].iter().collect::<String>().trim().to_string());
        rest = chars[split_at..]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
    }

    parts.push(rest);
    parts
}

pub fn parse_command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split('@')
        .next()
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub fn command_arg(text: &str) -> String {
    let mut parts = text.splitn(2, char::is_whitespace);
    let _command = parts.next();
    parts.next().unwrap_or("").trim().to_string()
}

pub fn session_label(session_id: &str) -> String {
    if session_id.is_empty() {
        return "none".to_string();
    }
    session_id.chars().take(8).collect()
}

pub fn log_line_count(text: &str) -> usize {
    text.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(200))
        .unwrap_or(80)
}

pub fn tail_log_text(text: &str, lines: usize) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "Gateway log is empty.".to_string();
    }
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

pub fn join_non_empty(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_telegram_message_keeps_parts_under_limit() {
        let text = "a".repeat(TELEGRAM_MESSAGE_LIMIT + 50);
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert!(
            parts
                .iter()
                .all(|part| part.chars().count() <= TELEGRAM_MESSAGE_LIMIT)
        );
    }

    #[test]
    fn split_telegram_message_prefers_recent_newline() {
        let text = format!(
            "{}\n{}",
            "a".repeat(TELEGRAM_MESSAGE_LIMIT - 20),
            "b".repeat(100)
        );
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].chars().last(), Some('a'));
        assert!(parts[1].starts_with('b'));
    }

    #[test]
    fn parse_command_strips_bot_suffix_and_lowercases() {
        assert_eq!(parse_command("/Log@MyBot 20"), Some("/log".to_string()));
        assert_eq!(parse_command("normal prompt"), None);
    }

    #[test]
    fn command_arg_preserves_spaces_after_command() {
        assert_eq!(command_arg("/rename work session"), "work session");
        assert_eq!(command_arg("/list"), "");
    }

    #[test]
    fn session_label_uses_short_id_or_none() {
        assert_eq!(session_label(""), "none");
        assert_eq!(session_label("12345678"), "12345678");
        assert_eq!(session_label("123456789abc"), "12345678");
    }

    #[test]
    fn log_line_count_defaults_and_caps() {
        assert_eq!(log_line_count("/log"), 80);
        assert_eq!(log_line_count("/log 10"), 10);
        assert_eq!(log_line_count("/log bad"), 80);
        assert_eq!(log_line_count("/log 0"), 80);
        assert_eq!(log_line_count("/log 999"), 200);
    }

    #[test]
    fn tail_log_text_returns_last_lines() {
        assert_eq!(tail_log_text("one\ntwo\nthree\n", 2), "two\nthree");
        assert_eq!(tail_log_text("", 2), "Gateway log is empty.");
    }

    #[test]
    fn join_non_empty_trims_and_separates() {
        assert_eq!(join_non_empty(&[" hello ", "", " world "]), "hello\n\nworld");
    }
}
