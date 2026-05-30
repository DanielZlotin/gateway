use crate::session::ChatSession;
use crate::text::session_label;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn status_header(state: &ChatSession) -> String {
    format!(
        "Model: {}\nSession: {}",
        state.model,
        session_label(state.session_id.as_deref().unwrap_or(""))
    )
}

pub fn format_status_message(state: &ChatSession, fetch: &str) -> String {
    let fetch = fetch.trim();
    if fetch.is_empty() {
        return status_header(state);
    }
    format!("{}\n\n{fetch}", status_header(state))
}

pub fn fastfetch_status(bin: &Path) -> String {
    let output = Command::new(bin).output();
    match output {
        Ok(output) if output.status.success() => {
            let text = format_fastfetch_output(&String::from_utf8_lossy(&output.stdout));
            if text.is_empty() {
                "fastfetch: no output".to_string()
            } else {
                text
            }
        }
        Ok(output) => format!("fastfetch: exited with {}", output.status),
        Err(err) => format!("fastfetch: {err}"),
    }
}

pub fn format_fastfetch_output(raw: &str) -> String {
    let mut lines = Vec::new();
    for raw_line in strip_ansi(raw).lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('-') || !line.contains(':') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let Some(emoji) = fastfetch_emoji(key) else {
            continue;
        };
        lines.push(format!("• {emoji} {key}: {value}"));
    }
    lines.join("\n")
}

fn fastfetch_emoji(key: &str) -> Option<&'static str> {
    match key {
        "OS" => Some("🖥️"),
        "Host" => Some("💻"),
        "Kernel" => Some("⚙️"),
        "Uptime" => Some("⏱️"),
        "CPU" => Some("🧠"),
        "GPU" => Some("🎮"),
        "Memory" => Some("💾"),
        "Swap" => Some("🔁"),
        key if key.starts_with("Disk") => Some("🗄️"),
        key if key.starts_with("Local IP") => Some("🌐"),
        key if key.starts_with("Battery") => Some("🔋"),
        "Power Adapter" => Some("🔌"),
        _ => None,
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        for next in chars.by_ref() {
            if ('@'..='~').contains(&next) {
                break;
            }
        }
    }
    out
}

pub fn typing_interval() -> Duration {
    Duration::from_secs(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_header_does_not_include_command_help() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = status_header(&state);

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("Session: 12345678"));
        assert!(!got.contains("/commands"));
    }

    #[test]
    fn format_status_message_appends_fetch() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = format_status_message(&state, "OS: test");

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("OS: test"));
        assert!(!got.contains("Gateway restarted."));
    }

    #[test]
    fn fastfetch_output_formats_telegram_bullets() {
        let raw = "\x1b[34C------------------------------\n\x1b[34COS: macOS Tahoe 26.3\n\x1b[34CHost: MacBook Pro\n\x1b[34CPackages: 126\n\x1b[34CMemory: 8.60 GiB / 64.00 GiB (13%)\n";

        let got = format_fastfetch_output(raw);

        assert_eq!(
            got,
            "• 🖥️ OS: macOS Tahoe 26.3\n• 💻 Host: MacBook Pro\n• 💾 Memory: 8.60 GiB / 64.00 GiB (13%)"
        );
    }
}
