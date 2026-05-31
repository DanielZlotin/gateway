pub const DIRECTIVES: &str =
    "/commands, /help, /status, /log, /new, /restart, /model, /resume, /rename, /list";

pub fn directive_help() -> String {
    [
        "Supported directives:",
        "/commands - show supported gateway directives",
        "/help - alias for /commands",
        "/status - show Codex, gateway, and system status",
        "/log [lines] - send recent gateway logs",
        "/new - start a fresh Codex session",
        "/restart - restart the gateway service",
        "/model [name] - show or set the Codex model",
        "/resume SESSION_OR_NAME - resume a saved session",
        "/rename NAME - rename the current session",
        "/list - list saved sessions",
    ]
    .join("\n")
}

pub fn unknown_directive_message() -> String {
    format!("Unknown directive. Defined directives: {DIRECTIVES}")
}

pub fn is_allowed(telegram_chat_ids: &[i64], chat_id: i64) -> bool {
    telegram_chat_ids.contains(&chat_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_help_includes_supported_commands() {
        let help = directive_help();
        for command in [
            "/commands",
            "/help",
            "/status",
            "/log",
            "/new",
            "/restart",
            "/model",
            "/resume",
            "/rename",
            "/list",
        ] {
            assert!(help.contains(command), "missing {command}");
        }
        assert!(!help.contains("/start"));
    }

    #[test]
    fn unknown_directive_mentions_defined_directives() {
        let message = unknown_directive_message();
        assert!(message.contains("Unknown directive."));
        assert!(message.contains(DIRECTIVES));
    }

    #[test]
    fn is_allowed_accepts_only_allowed_chat_id() {
        assert!(is_allowed(&[42], 42));
        assert!(!is_allowed(&[42], 7));
    }
}
