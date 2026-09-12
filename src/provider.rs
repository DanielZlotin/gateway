use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Codex,
    Claude,
    Openrouter,
}

impl Provider {
    pub fn model_label(self, model: &str) -> &str {
        if self == Self::Codex && model.trim().is_empty() {
            "Codex default (inherited)"
        } else {
            model
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Openrouter => "openrouter",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Openrouter => "OpenRouter",
        }
    }
}
