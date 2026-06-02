pub const TELEGRAM_MESSAGE_LIMIT: usize = 3900;
pub const DEFAULT_LOG_LINES: usize = 10;

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

        parts.push(
            chars[..split_at]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        );
        rest = chars[split_at..]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
    }

    parts.push(rest);
    parts
}

pub fn redact_private_data(text: &str) -> String {
    let mut redacted = Vec::new();
    let mut in_private_key = false;

    for line in text.lines() {
        if is_private_key_boundary(line, "begin") {
            redacted.push("-----BEGIN <redacted> PRIVATE KEY-----".to_string());
            in_private_key = true;
            continue;
        }
        if in_private_key {
            if is_private_key_boundary(line, "end") {
                in_private_key = false;
            }
            continue;
        }
        redacted.push(redact_sensitive_line(line));
    }

    let mut out = redacted.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn redact_sensitive_line(line: &str) -> String {
    let line = redact_bearer_token(line);
    let line = redact_sensitive_assignment(&line);
    redact_prefixed_tokens(&line)
}

fn redact_bearer_token(line: &str) -> String {
    let Some(start) = find_ascii_case_insensitive(line, "Bearer ") else {
        return line.to_string();
    };
    let token_start = start + "Bearer ".len();
    let token_end = token_end(line, token_start);
    if token_start == token_end {
        return line.to_string();
    }
    format!("{}<redacted>{}", &line[..token_start], &line[token_end..])
}

fn redact_sensitive_assignment(line: &str) -> String {
    if line.contains("<redacted>") {
        return line.to_string();
    }
    let Some((index, separator)) = line
        .char_indices()
        .find(|(_, ch)| matches!(ch, '=' | ':'))
        .map(|(index, ch)| (index, ch))
    else {
        return line.to_string();
    };
    let key = line[..index]
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'));
    if !is_sensitive_key(key) {
        return line.to_string();
    }

    let value = &line[index + separator.len_utf8()..];
    let spacing: String = value.chars().take_while(|ch| ch.is_whitespace()).collect();
    format!("{}{}{}<redacted>", &line[..index], separator, spacing)
}

fn redact_prefixed_tokens(line: &str) -> String {
    let mut out = line.to_string();
    for prefix in [
        "sk-",
        "sk-ant-",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
    ] {
        out = redact_tokens_with_prefix(&out, prefix);
    }
    out
}

fn redact_tokens_with_prefix(line: &str, prefix: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(index) = rest.find(prefix) {
        out.push_str(&rest[..index]);
        let token_start = index;
        let token_end = token_end(rest, token_start);
        if token_end - token_start >= prefix.len() + 8 {
            out.push_str("<redacted>");
        } else {
            out.push_str(&rest[token_start..token_end]);
        }
        rest = &rest[token_end..];
    }
    out.push_str(rest);
    out
}

fn token_end(text: &str, start: usize) -> usize {
    text[start..]
        .char_indices()
        .find_map(|(offset, ch)| {
            (ch.is_whitespace() || matches!(ch, ',' | ';' | '"' | '\'' | '`' | '<' | '>'))
                .then_some(start + offset)
        })
        .unwrap_or(text.len())
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "auth_header",
        "cookie",
        "password",
        "private_key",
        "secret",
        "seed_phrase",
        "session_token",
        "token",
    ]
    .iter()
    .any(|part| normalized.contains(part))
}

fn is_private_key_boundary(line: &str, boundary: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains(&format!("-----{boundary}"))
        && lower.contains("private key")
        && lower.contains("-----")
}

fn find_ascii_case_insensitive(text: &str, needle: &str) -> Option<usize> {
    text.to_ascii_lowercase().find(&needle.to_ascii_lowercase())
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
    normalize_log_line_count(
        text.split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<usize>().ok()),
    )
}

pub fn normalize_log_line_count(lines: Option<usize>) -> usize {
    lines
        .filter(|value| *value > 0)
        .map(|value| value.min(200))
        .unwrap_or(DEFAULT_LOG_LINES)
}

pub fn tail_log_text(text: &str, lines: usize) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "📭 Gateway log is empty.".to_string();
    }
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n\n")
}

pub fn tail_log_plain_text(text: &str, lines: usize) -> String {
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

pub fn is_ok_response(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("OK")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_telegram_message_keeps_parts_under_limit() {
        let text = "a".repeat(TELEGRAM_MESSAGE_LIMIT + 50);
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert!(parts
            .iter()
            .all(|part| part.chars().count() <= TELEGRAM_MESSAGE_LIMIT));
    }

    #[test]
    fn split_telegram_message_returns_empty_part_for_empty_text() {
        assert_eq!(split_telegram_message(" \n\t "), vec![String::new()]);
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
        assert_eq!(log_line_count("/log"), 10);
        assert_eq!(log_line_count("/log 10"), 10);
        assert_eq!(log_line_count("/log bad"), 10);
        assert_eq!(log_line_count("/log 0"), 10);
        assert_eq!(log_line_count("/log 999"), 200);
        assert_eq!(normalize_log_line_count(None), 10);
        assert_eq!(normalize_log_line_count(Some(0)), 10);
        assert_eq!(normalize_log_line_count(Some(999)), 200);
    }

    #[test]
    fn tail_log_text_returns_last_lines() {
        assert_eq!(tail_log_text("one\ntwo\nthree\n", 2), "two\n\nthree");
        assert_eq!(tail_log_text("", 2), "📭 Gateway log is empty.");
    }

    #[test]
    fn join_non_empty_trims_and_separates() {
        assert_eq!(
            join_non_empty(&[" hello ", "", " world "]),
            "hello\n\nworld"
        );
    }

    #[test]
    fn is_ok_response_trims_and_ignores_case() {
        assert!(is_ok_response("OK"));
        assert!(is_ok_response(" ok\n"));
        assert!(is_ok_response("oK"));
        assert!(!is_ok_response(""));
        assert!(!is_ok_response("OK done"));
    }
}
