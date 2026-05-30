pub const DIRECTIVES: &str =
    "/commands, /help, /status, /log, /new, /restart, /model, /resume, /rename, /list";

pub fn directive_help() -> String {
    [
        "Supported directives:",
        "/commands - show supported gateway directives",
        "/help - alias for /commands",
        "/status - show gateway status and system snapshot",
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

pub fn is_allowed(allowed_ids: &[i64], chat_id: i64, from_id: Option<i64>) -> bool {
    allowed_ids.contains(&chat_id) || from_id.is_some_and(|id| allowed_ids.contains(&id))
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
    fn is_allowed_accepts_chat_or_sender() {
        assert!(is_allowed(&[42], 42, None));
        assert!(is_allowed(&[42], 7, Some(42)));
        assert!(!is_allowed(&[42], 7, Some(8)));
    }
}
