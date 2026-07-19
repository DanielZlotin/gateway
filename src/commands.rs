#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    Status,
    Heartbeat,
    Log,
    New,
    Restart,
    Update,
    Model,
    Resume,
    Rename,
    List,
    Stop,
    Voice,
}

impl Directive {
    pub fn command(self) -> &'static str {
        match self {
            Directive::Status => "status",
            Directive::Heartbeat => "heartbeat",
            Directive::Log => "log",
            Directive::New => "new",
            Directive::Restart => "restart",
            Directive::Update => "update",
            Directive::Model => "model",
            Directive::Resume => "resume",
            Directive::Rename => "rename",
            Directive::List => "list",
            Directive::Stop => "stop",
            Directive::Voice => "voice",
        }
    }
}

pub struct DirectiveSpec {
    pub directive: Directive,
    icon: &'static str,
    usage: &'static str,
    summary: &'static str,
}

impl DirectiveSpec {
    pub fn command(&self) -> &'static str {
        self.directive.command()
    }

    pub fn bot_description(&self) -> String {
        format!("{} {}.", self.icon, sentence_case(self.summary))
    }

    fn readme_line(&self) -> String {
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
        directive: Directive::Voice,
        icon: "🔊",
        usage: " [on|off]",
        summary: "toggle spoken audio replies",
    },
    DirectiveSpec {
        directive: Directive::Update,
        icon: "📦",
        usage: "",
        summary: "update gateway, tools, and setup",
    },
    DirectiveSpec {
        directive: Directive::New,
        icon: "✨",
        usage: "",
        summary: "start a fresh Codex session",
    },
    DirectiveSpec {
        directive: Directive::Status,
        icon: "📊",
        usage: "",
        summary: "show Codex, gateway, and system status",
    },
    DirectiveSpec {
        directive: Directive::List,
        icon: "📚",
        usage: "",
        summary: "list saved sessions",
    },
    DirectiveSpec {
        directive: Directive::Resume,
        icon: "↩️",
        usage: " [SESSION_OR_NAME|index]",
        summary: "list or resume a saved session",
    },
    DirectiveSpec {
        directive: Directive::Rename,
        icon: "🏷️",
        usage: " [NAME]",
        summary: "rename the current session",
    },
    DirectiveSpec {
        directive: Directive::Model,
        icon: "🧠",
        usage: " [index]",
        summary: "choose a configured provider/model",
    },
    DirectiveSpec {
        directive: Directive::Heartbeat,
        icon: "🫀",
        usage: "",
        summary: "run heartbeat and print result",
    },
    DirectiveSpec {
        directive: Directive::Log,
        icon: "📜",
        usage: " [lines]",
        summary: "send recent gateway logs",
    },
    DirectiveSpec {
        directive: Directive::Restart,
        icon: "🔁",
        usage: "",
        summary: "restart the gateway service",
    },
    DirectiveSpec {
        directive: Directive::Stop,
        icon: "🛑",
        usage: "",
        summary: "cancel this chat's Codex work",
    },
];

pub fn readme_command_lines() -> Vec<String> {
    DIRECTIVE_SPECS
        .iter()
        .map(DirectiveSpec::readme_line)
        .collect()
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

fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut sentence = first.to_uppercase().collect::<String>();
    sentence.push_str(chars.as_str());
    sentence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_list_includes_supported_commands() {
        let list = directive_list();

        assert!(list.contains("/status"));
        assert!(list.contains("/heartbeat"));
        assert!(list.contains("/update"));
        assert!(list.contains("/stop"));
        assert!(list.contains("/voice"));
        assert!(!list.contains("/config"));
        assert!(!list.contains("/help"));
        assert!(!list.contains("/commands"));
        assert!(!list.contains("/start"));
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
        assert_eq!(directive_from_command("/config"), None);
        assert_eq!(directive_from_command("/help"), None);
        assert_eq!(directive_from_command("/commands"), None);
    }

    #[test]
    fn unknown_directive_mentions_defined_directives() {
        let message = unknown_directive_message();
        assert!(message.contains("❓ Unknown directive."));
        assert!(message.contains(&directive_list()));
    }

    #[test]
    fn command_summaries_are_six_words_or_less() {
        for spec in DIRECTIVE_SPECS {
            let word_count = spec.summary.split_whitespace().count();
            assert!(
                word_count <= 6,
                "/{} summary has {word_count} words: {}",
                spec.command(),
                spec.summary
            );
        }
    }

    #[test]
    fn readme_telegram_command_block_matches_specs() {
        let readme_commands = include_str!("../README.md")
            .split_once("```text\n")
            .and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(block, _)| block.lines().map(str::to_string).collect::<Vec<String>>())
            .expect("README must include a Telegram command block");

        assert_eq!(readme_commands, readme_command_lines());
    }

    #[test]
    fn is_allowed_accepts_only_allowed_chat_id() {
        assert!(is_allowed(&[42], 42));
        assert!(!is_allowed(&[42], 7));
    }
}
