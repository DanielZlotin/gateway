pub struct DirectiveSpec {
    pub command: &'static str,
    help_line: &'static str,
    pub bot_description: &'static str,
}

pub const DIRECTIVE_SPECS: &[DirectiveSpec] = &[
    DirectiveSpec {
        command: "help",
        help_line: "❔ /help - show supported gateway directives",
        bot_description: "❔ Show supported gateway directives.",
    },
    DirectiveSpec {
        command: "status",
        help_line: "📊 /status - show Codex, gateway, and system status",
        bot_description: "📊 Show Codex, gateway, and system status.",
    },
    DirectiveSpec {
        command: "config",
        help_line: "⚙️ /config - show loaded gateway config with secrets redacted",
        bot_description: "⚙️ Show loaded gateway config with secrets redacted.",
    },
    DirectiveSpec {
        command: "log",
        help_line: "📜 /log [lines] - send recent gateway logs",
        bot_description: "📜 Send recent gateway logs.",
    },
    DirectiveSpec {
        command: "new",
        help_line: "🆕 /new - start a fresh Codex session",
        bot_description: "🆕 Start a fresh Codex session.",
    },
    DirectiveSpec {
        command: "restart",
        help_line: "🔄 /restart - restart the gateway service",
        bot_description: "🔄 Restart the gateway service.",
    },
    DirectiveSpec {
        command: "model",
        help_line: "🤖 /model [index] - choose a configured provider/model",
        bot_description: "🤖 Choose a configured provider/model.",
    },
    DirectiveSpec {
        command: "resume",
        help_line: "↩️ /resume [SESSION_OR_NAME|index] - list or resume a saved session",
        bot_description: "↩️ Resume a saved session.",
    },
    DirectiveSpec {
        command: "rename",
        help_line: "🏷️ /rename [NAME] - rename the current session",
        bot_description: "🏷️ Rename the current session.",
    },
    DirectiveSpec {
        command: "list",
        help_line: "💾 /list - list saved sessions",
        bot_description: "💾 List saved sessions.",
    },
];

pub fn directive_help() -> String {
    std::iter::once("🧭 Supported directives:")
        .chain(DIRECTIVE_SPECS.iter().map(|spec| spec.help_line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn unknown_directive_message() -> String {
    format!(
        "❓ Unknown directive. Defined directives: {}",
        directive_list()
    )
}

fn directive_list() -> String {
    DIRECTIVE_SPECS
        .iter()
        .map(|spec| format!("/{}", spec.command))
        .collect::<Vec<_>>()
        .join(", ")
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
        assert!(help.starts_with("🧭 Supported directives:"));
        for command in directive_list().split(", ") {
            assert!(help.contains(command), "missing {command}");
        }
        assert!(help.contains("📊 /status"));
        assert!(help.contains("⚙️ /config"));
        assert!(!help.contains("/commands"));
        assert!(!help.contains("/start"));
    }

    #[test]
    fn unknown_directive_mentions_defined_directives() {
        let message = unknown_directive_message();
        assert!(message.contains("❓ Unknown directive."));
        assert!(message.contains(&directive_list()));
    }

    #[test]
    fn is_allowed_accepts_only_allowed_chat_id() {
        assert!(is_allowed(&[42], 42));
        assert!(!is_allowed(&[42], 7));
    }
}
