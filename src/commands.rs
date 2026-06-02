#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    Help,
    Status,
    Config,
    Log,
    New,
    Restart,
    Update,
    Model,
    Resume,
    Rename,
    List,
}

impl Directive {
    pub fn command(self) -> &'static str {
        match self {
            Directive::Help => "help",
            Directive::Status => "status",
            Directive::Config => "config",
            Directive::Log => "log",
            Directive::New => "new",
            Directive::Restart => "restart",
            Directive::Update => "update",
            Directive::Model => "model",
            Directive::Resume => "resume",
            Directive::Rename => "rename",
            Directive::List => "list",
        }
    }
}

pub struct DirectiveSpec {
    pub directive: Directive,
    icon: &'static str,
    usage: &'static str,
    summary: &'static str,
    pub bot_description: &'static str,
}

impl DirectiveSpec {
    pub fn command(&self) -> &'static str {
        self.directive.command()
    }

    fn help_line(&self) -> String {
        format!(
            "{} /{}{} - {}",
            self.icon,
            self.command(),
            self.usage,
            self.summary
        )
    }
}

pub const DIRECTIVE_SPECS: &[DirectiveSpec] = &[
    DirectiveSpec {
        directive: Directive::Help,
        icon: "❔",
        usage: "",
        summary: "show supported gateway directives",
        bot_description: "❔ Show supported gateway directives.",
    },
    DirectiveSpec {
        directive: Directive::Status,
        icon: "📊",
        usage: "",
        summary: "show Codex, gateway, and system status",
        bot_description: "📊 Show Codex, gateway, and system status.",
    },
    DirectiveSpec {
        directive: Directive::Config,
        icon: "⚙️",
        usage: "",
        summary: "show loaded gateway config with secrets redacted",
        bot_description: "⚙️ Show loaded gateway config with secrets redacted.",
    },
    DirectiveSpec {
        directive: Directive::Log,
        icon: "📜",
        usage: " [lines]",
        summary: "send recent gateway logs",
        bot_description: "📜 Send recent gateway logs.",
    },
    DirectiveSpec {
        directive: Directive::New,
        icon: "🆕",
        usage: "",
        summary: "start a fresh Codex session",
        bot_description: "🆕 Start a fresh Codex session.",
    },
    DirectiveSpec {
        directive: Directive::Restart,
        icon: "🔄",
        usage: "",
        summary: "restart the gateway service",
        bot_description: "🔄 Restart the gateway service.",
    },
    DirectiveSpec {
        directive: Directive::Update,
        icon: "⬆️",
        usage: "",
        summary: "pull latest gateway code and run setup",
        bot_description: "⬆️ Pull latest gateway code and run setup.",
    },
    DirectiveSpec {
        directive: Directive::Model,
        icon: "🤖",
        usage: " [index]",
        summary: "choose a configured provider/model",
        bot_description: "🤖 Choose a configured provider/model.",
    },
    DirectiveSpec {
        directive: Directive::Resume,
        icon: "↩️",
        usage: " [SESSION_OR_NAME|index]",
        summary: "list or resume a saved session",
        bot_description: "↩️ Resume a saved session.",
    },
    DirectiveSpec {
        directive: Directive::Rename,
        icon: "🏷️",
        usage: " [NAME]",
        summary: "rename the current session",
        bot_description: "🏷️ Rename the current session.",
    },
    DirectiveSpec {
        directive: Directive::List,
        icon: "💾",
        usage: "",
        summary: "list saved sessions",
        bot_description: "💾 List saved sessions.",
    },
];

pub fn directive_help() -> String {
    std::iter::once("🧭 Supported directives:".to_string())
        .chain(DIRECTIVE_SPECS.iter().map(DirectiveSpec::help_line))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn directive_from_command(command: &str) -> Option<Directive> {
    let command = command.strip_prefix('/').unwrap_or(command);
    DIRECTIVE_SPECS
        .iter()
        .find(|spec| spec.command() == command)
        .map(|spec| spec.directive)
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
        .map(|spec| format!("/{}", spec.command()))
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
        assert!(help.contains("⬆️ /update"));
        assert!(!help.contains("/commands"));
        assert!(!help.contains("/start"));
    }

    #[test]
    fn directive_lookup_matches_advertised_directives() {
        for spec in DIRECTIVE_SPECS {
            assert_eq!(
                directive_from_command(&format!("/{}", spec.command())),
                Some(spec.directive),
                "advertised /{} must resolve to its directive",
                spec.command()
            );
        }
        assert_eq!(directive_from_command("/commands"), None);
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
