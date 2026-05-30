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
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
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
}
