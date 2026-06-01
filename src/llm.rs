use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ApiProviderConfig {
    pub anthropic_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub claude_model: String,
    pub openrouter_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiOutput {
    pub final_text: String,
}

impl From<&Config> for ApiProviderConfig {
    fn from(cfg: &Config) -> Self {
        Self {
            anthropic_api_key: cfg.anthropic_api_key.clone(),
            openrouter_api_key: cfg.openrouter_api_key.clone(),
            claude_model: cfg.claude_model.clone(),
            openrouter_model: cfg.openrouter_model.clone(),
        }
    }
}

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const MAX_TOKENS: u64 = 4096;

pub fn run_claude(
    cfg: &ApiProviderConfig,
    prompt: &str,
    timeout: Duration,
) -> Result<ApiOutput, String> {
    let api_key = cfg
        .anthropic_api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "ANTHROPIC_API_KEY is required for Claude.".to_string())?;
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let body = AnthropicRequest {
        model: cfg.claude_model.as_str(),
        max_tokens: MAX_TOKENS,
        messages: vec![AnthropicMessage {
            role: "user",
            content: prompt,
        }],
    };
    let response = agent
        .post(ANTHROPIC_URL)
        .set("x-api-key", api_key)
        .set("anthropic-version", "2023-06-01")
        .send_json(serde_json::to_value(body).map_err(|err| err.to_string())?);
    let value: AnthropicResponse = decode_json(response, "Claude")?;
    let final_text = value
        .content
        .into_iter()
        .filter_map(|part| match part {
            AnthropicContent::Text { text } => Some(text),
            AnthropicContent::Other => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    Ok(ApiOutput { final_text })
}

pub fn run_openrouter(
    cfg: &ApiProviderConfig,
    prompt: &str,
    timeout: Duration,
) -> Result<ApiOutput, String> {
    let api_key = cfg
        .openrouter_api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "OPENROUTER_API_KEY is required for OpenRouter.".to_string())?;
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let body = OpenRouterRequest {
        model: cfg.openrouter_model.as_str(),
        messages: vec![OpenRouterMessage {
            role: "user",
            content: prompt,
        }],
    };
    let response = agent
        .post(OPENROUTER_URL)
        .set("Authorization", &format!("Bearer {api_key}"))
        .send_json(serde_json::to_value(body).map_err(|err| err.to_string())?);
    let value: OpenRouterResponse = decode_json(response, "OpenRouter")?;
    let final_text = value
        .choices
        .into_iter()
        .filter_map(|choice| choice.message.content)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    Ok(ApiOutput { final_text })
}

fn decode_json<T: for<'de> Deserialize<'de>>(
    response: Result<ureq::Response, ureq::Error>,
    label: &str,
) -> Result<T, String> {
    match response {
        Ok(response) => response
            .into_json()
            .map_err(|err| format!("{label} response decode failed: {err}")),
        Err(ureq::Error::Status(status, response)) => {
            let text = response.into_string().unwrap_or_default();
            Err(format!(
                "{label} request failed with status {status}: {text}"
            ))
        }
        Err(err) => Err(format!("{label} request failed: {err}")),
    }
}

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u64,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Serialize)]
struct OpenRouterRequest<'a> {
    model: &'a str,
    messages: Vec<OpenRouterMessage<'a>>,
}

#[derive(Serialize)]
struct OpenRouterMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct OpenRouterResponse {
    choices: Vec<OpenRouterChoice>,
}

#[derive(Deserialize)]
struct OpenRouterChoice {
    message: OpenRouterChoiceMessage,
}

#[derive(Deserialize)]
struct OpenRouterChoiceMessage {
    content: Option<String>,
}
