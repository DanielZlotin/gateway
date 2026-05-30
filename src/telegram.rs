use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommandScope {
    #[serde(rename = "type")]
    pub scope_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandScopeTarget {
    pub name: String,
    pub scope: BotCommandScope,
    pub set: bool,
}

#[derive(Debug, Deserialize)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub caption: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub username: String,
}

#[derive(Clone)]
pub struct TelegramClient {
    base_url: String,
    agent: ureq::Agent,
}

impl TelegramClient {
    pub fn new(token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(65))
            .build();
        Self {
            base_url: format!("https://api.telegram.org/bot{token}"),
            agent,
        }
    }

    pub fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String> {
        let mut request = self
            .agent
            .get(&format!("{}/getUpdates", self.base_url))
            .query("timeout", &timeout_sec.to_string())
            .query("allowed_updates", r#"["message"]"#);
        if offset > 0 {
            request = request.query("offset", &offset.to_string());
        }
        self.call_json(request.call(), "getUpdates")
    }

    pub fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<(), String> {
        let mut values = vec![
            ("chat_id", chat_id.to_string()),
            ("text", text.to_string()),
            ("disable_web_page_preview", "true".to_string()),
        ];
        if reply_to_message_id > 0 {
            values.push(("reply_to_message_id", reply_to_message_id.to_string()));
            values.push(("allow_sending_without_reply", "true".to_string()));
        }
        let _: serde_json::Value = self.post_form("sendMessage", &values)?;
        Ok(())
    }

    pub fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String> {
        let values = [
            ("chat_id", chat_id.to_string()),
            ("action", action.to_string()),
        ];
        let _: bool = self.post_form("sendChatAction", &values)?;
        Ok(())
    }

    pub fn sync_my_commands(&self, chat_ids: &[i64]) -> Result<(), String> {
        let languages = ["", "en", "he"];
        let targets = command_scope_targets(chat_ids);
        for target in &targets {
            for language in languages {
                self.delete_my_commands(&target.scope, language)?;
            }
        }
        for target in targets.iter().filter(|target| target.set) {
            for language in languages {
                self.set_my_commands(&target.scope, language)?;
            }
        }
        Ok(())
    }

    fn delete_my_commands(
        &self,
        scope: &BotCommandScope,
        language_code: &str,
    ) -> Result<(), String> {
        let values = command_request_values(scope, language_code)?;
        let refs = refs(&values);
        let _: bool = self.post_form("deleteMyCommands", &refs)?;
        Ok(())
    }

    fn set_my_commands(&self, scope: &BotCommandScope, language_code: &str) -> Result<(), String> {
        let mut values = command_request_values(scope, language_code)?;
        values.push((
            "commands".to_string(),
            serde_json::to_string(&supported_bot_commands()).map_err(|err| err.to_string())?,
        ));
        let refs = refs(&values);
        let _: bool = self.post_form("setMyCommands", &refs)?;
        Ok(())
    }

    fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        values: &[(&str, String)],
    ) -> Result<T, String> {
        let form: Vec<(&str, &str)> = values
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        self.call_json(
            self.agent
                .post(&format!("{}/{method}", self.base_url))
                .send_form(&form),
            method,
        )
    }

    fn call_json<T: serde::de::DeserializeOwned>(
        &self,
        response: Result<ureq::Response, ureq::Error>,
        method: &str,
    ) -> Result<T, String> {
        let response = response.map_err(|err| {
            format!(
                "telegram {method} request failed: {}",
                redact_token(&self.base_url, &err.to_string())
            )
        })?;
        let envelope: TelegramResponse<T> = response
            .into_json()
            .map_err(|err| format!("decode telegram {method} response: {err}"))?;
        if envelope.ok {
            envelope
                .result
                .ok_or_else(|| format!("telegram {method} returned no result"))
        } else {
            Err(envelope
                .description
                .unwrap_or_else(|| format!("telegram {method} failed")))
        }
    }
}

pub fn supported_bot_commands() -> Vec<BotCommand> {
    [
        ("commands", "Show supported gateway directives."),
        ("help", "Alias for /commands."),
        ("status", "Show gateway status and system snapshot."),
        ("log", "Send recent gateway logs."),
        ("new", "Start a fresh Codex session."),
        ("restart", "Restart the gateway service."),
        ("model", "Show or set the Codex model."),
        ("resume", "Resume a saved session."),
        ("rename", "Rename the current session."),
        ("list", "List saved sessions."),
    ]
    .into_iter()
    .map(|(command, description)| BotCommand {
        command: command.to_string(),
        description: description.to_string(),
    })
    .collect()
}

pub fn command_scope_targets(chat_ids: &[i64]) -> Vec<CommandScopeTarget> {
    let mut targets = vec![
        target("default", "default", None, true),
        target("all_private_chats", "all_private_chats", None, true),
        target("all_group_chats", "all_group_chats", None, false),
        target(
            "all_chat_administrators",
            "all_chat_administrators",
            None,
            false,
        ),
    ];
    for chat_id in chat_ids {
        targets.push(target(
            &format!("chat:{chat_id}"),
            "chat",
            Some(*chat_id),
            true,
        ));
    }
    targets
}

pub fn command_request_values(
    scope: &BotCommandScope,
    language_code: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut values = vec![(
        "scope".to_string(),
        serde_json::to_string(scope).map_err(|err| err.to_string())?,
    )];
    if !language_code.is_empty() {
        values.push(("language_code".to_string(), language_code.to_string()));
    }
    Ok(values)
}

pub fn redact_token(base_url: &str, value: &str) -> String {
    value.replace(base_url, "https://api.telegram.org/bot<redacted>")
}

fn target(name: &str, scope_type: &str, chat_id: Option<i64>, set: bool) -> CommandScopeTarget {
    CommandScopeTarget {
        name: name.to_string(),
        scope: BotCommandScope {
            scope_type: scope_type.to_string(),
            chat_id,
        },
        set,
    }
}

fn refs(values: &[(String, String)]) -> Vec<(&str, String)> {
    values
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_commands_match_directives() {
        let commands = supported_bot_commands();
        let names: Vec<_> = commands
            .iter()
            .map(|command| command.command.as_str())
            .collect();

        assert_eq!(
            names,
            vec![
                "commands", "help", "status", "log", "new", "restart", "model", "resume", "rename",
                "list"
            ]
        );
    }

    #[test]
    fn command_scope_targets_match_go_gateway() {
        let targets = command_scope_targets(&[42]);
        let summary: Vec<_> = targets
            .iter()
            .map(|target| {
                (
                    target.name.as_str(),
                    target.scope.scope_type.as_str(),
                    target.set,
                )
            })
            .collect();

        assert_eq!(
            summary,
            vec![
                ("default", "default", true),
                ("all_private_chats", "all_private_chats", true),
                ("all_group_chats", "all_group_chats", false),
                ("all_chat_administrators", "all_chat_administrators", false),
                ("chat:42", "chat", true),
            ]
        );
        assert_eq!(targets[4].scope.chat_id, Some(42));
    }

    #[test]
    fn command_request_values_encode_scope_and_language() {
        let values = command_request_values(
            &BotCommandScope {
                scope_type: "chat".to_string(),
                chat_id: Some(42),
            },
            "en",
        )
        .unwrap();

        assert!(values.contains(&(
            "scope".to_string(),
            r#"{"type":"chat","chat_id":42}"#.to_string()
        )));
        assert!(values.contains(&("language_code".to_string(), "en".to_string())));
    }

    #[test]
    fn redact_token_hides_base_url() {
        let base = "https://api.telegram.org/botsecret";
        assert_eq!(
            redact_token(
                base,
                "request to https://api.telegram.org/botsecret/getUpdates failed"
            ),
            "request to https://api.telegram.org/bot<redacted>/getUpdates failed"
        );
    }
}
