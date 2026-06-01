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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Openrouter => "OpenRouter",
        }
    }

    pub const fn command(self) -> &'static str {
        match self {
            Self::Codex => "/codex",
            Self::Claude => "/claude",
            Self::Openrouter => "/openrouter",
        }
    }
}

pub fn provider_for_model_slot(slot: &str) -> Option<Provider> {
    match slot.trim() {
        "1" => Some(Provider::Codex),
        "2" => Some(Provider::Claude),
        "3" => Some(Provider::Openrouter),
        _ => None,
    }
}
