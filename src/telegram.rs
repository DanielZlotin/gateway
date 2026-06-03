use crate::commands::DIRECTIVE_SPECS;
use crate::text::redact_private_data;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::Path;
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
struct TelegramFile {
    #[serde(default)]
    file_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    #[serde(default)]
    pub effect_id: Option<String>,
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub caption: String,
    #[serde(default)]
    pub photo: Vec<PhotoSize>,
    #[serde(default)]
    pub document: Option<Document>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Document {
    pub file_id: String,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub mime_type: String,
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub data: String,
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
    file_base_url: String,
    agent: ureq::Agent,
}

const TELEGRAM_PARSE_MODE: &str = "Markdown";

impl TelegramClient {
    pub fn new(token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(65))
            .build();
        Self {
            base_url: format!("https://api.telegram.org/bot{token}"),
            file_base_url: format!("https://api.telegram.org/file/bot{token}"),
            agent,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build();
        Self {
            file_base_url: file_base_url_for(&base_url),
            base_url,
            agent,
        }
    }

    pub fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String> {
        let mut request = self
            .agent
            .get(&format!("{}/getUpdates", self.base_url))
            .query("timeout", &timeout_sec.to_string())
            .query("allowed_updates", r#"["message","callback_query"]"#);
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
        let _: serde_json::Value =
            self.post_send_message(chat_id, text, reply_to_message_id, None)?;
        Ok(())
    }

    pub fn send_message_with_inline_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        buttons: &[InlineKeyboardButton],
    ) -> Result<(), String> {
        let _: serde_json::Value =
            self.post_redacted_text_form("sendMessage", text, |text, parse_markdown| {
                send_message_with_inline_keyboard_values(
                    chat_id,
                    text,
                    reply_to_message_id,
                    buttons,
                    parse_markdown,
                )
            })?;
        Ok(())
    }

    pub fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<(), String> {
        let text = redact_private_data(text);
        let mut values = vec![("callback_query_id", callback_query_id.to_string())];
        if !text.trim().is_empty() {
            values.push(("text", text));
        }
        let _: bool = self.post_form("answerCallbackQuery", &values)?;
        Ok(())
    }

    pub fn send_message_returning(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<i64, String> {
        let message: Message = self.post_send_message(chat_id, text, reply_to_message_id, None)?;
        Ok(message.message_id)
    }

    pub fn send_message_with_effect(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        message_effect_id: &str,
    ) -> Result<Message, String> {
        self.post_send_message(chat_id, text, reply_to_message_id, Some(message_effect_id))
    }

    pub fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), String> {
        let values = [
            ("chat_id", chat_id.to_string()),
            ("message_id", message_id.to_string()),
        ];
        let _: bool = self.post_form("deleteMessage", &values)?;
        Ok(())
    }

    pub fn set_message_reaction(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: &str,
    ) -> Result<(), String> {
        let values = set_message_reaction_values(chat_id, message_id, emoji);
        let _: bool = self.post_form("setMessageReaction", &values)?;
        Ok(())
    }

    pub fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> Result<(), String> {
        let _: serde_json::Value =
            self.post_redacted_text_form("editMessageText", text, |text, parse_markdown| {
                Ok(edit_message_values(
                    chat_id,
                    message_id,
                    text,
                    parse_markdown,
                ))
            })?;
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

    pub fn download_file(&self, file_id: &str, path: &Path) -> Result<(), String> {
        let values = [("file_id", file_id.to_string())];
        let file: TelegramFile = self.post_form("getFile", &values)?;
        let file_path = file
            .file_path
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "telegram getFile returned no file_path".to_string())?;
        let url = format!("{}/{}", self.file_base_url, file_path.trim_start_matches('/'));
        let response = self.agent.get(&url).call().map_err(|err| {
            format!(
                "telegram download file request failed: {}",
                redact_file_token(&self.file_base_url, &err.to_string())
            )
        })?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|err| format!("read telegram file download: {err}"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create download dir: {err}"))?;
        }
        fs::write(path, bytes).map_err(|err| format!("write telegram file download: {err}"))
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

    fn post_form<T: DeserializeOwned>(
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

    fn post_form_with_plain_text_retry<T: DeserializeOwned>(
        &self,
        method: &str,
        values: &[(&str, String)],
        plain_values: &[(&str, String)],
    ) -> Result<T, String> {
        match self.post_form(method, values) {
            Ok(value) => Ok(value),
            Err(err) if should_retry_plain_text(&err) => self.post_form(method, plain_values),
            Err(err) => Err(err),
        }
    }

    fn post_send_message<T: DeserializeOwned>(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        message_effect_id: Option<&str>,
    ) -> Result<T, String> {
        self.post_redacted_text_form("sendMessage", text, |text, parse_markdown| {
            Ok(send_message_values(
                chat_id,
                text,
                reply_to_message_id,
                message_effect_id,
                parse_markdown,
            ))
        })
    }

    fn post_redacted_text_form<T: DeserializeOwned>(
        &self,
        method: &str,
        text: &str,
        build_values: impl Fn(&str, bool) -> Result<Vec<(&'static str, String)>, String>,
    ) -> Result<T, String> {
        let text = redact_private_data(text);
        let values = build_values(&text, true)?;
        let plain_values = build_values(&text, false)?;
        self.post_form_with_plain_text_retry(method, &values, &plain_values)
    }

    fn call_json<T: DeserializeOwned>(
        &self,
        response: Result<ureq::Response, ureq::Error>,
        method: &str,
    ) -> Result<T, String> {
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(status, response)) => {
                let envelope: Result<TelegramResponse<T>, _> = response.into_json();
                return match envelope {
                    Ok(envelope) => Err(envelope.description.unwrap_or_else(|| {
                        format!("telegram {method} failed with status {status}")
                    })),
                    Err(err) => Err(format!(
                        "telegram {method} request failed with status {status}; decode error response: {err}"
                    )),
                };
            }
            Err(err) => {
                return Err(format!(
                    "telegram {method} request failed: {}",
                    redact_token(&self.base_url, &err.to_string())
                ));
            }
        };
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InlineKeyboardButton {
    pub text: String,
    pub callback_data: String,
}

fn edit_message_values(
    chat_id: i64,
    message_id: i64,
    text: &str,
    parse_markdown: bool,
) -> Vec<(&'static str, String)> {
    let mut values = vec![
        ("chat_id", chat_id.to_string()),
        ("message_id", message_id.to_string()),
        ("text", text.to_string()),
        ("disable_web_page_preview", "true".to_string()),
    ];
    if parse_markdown {
        values.push(("parse_mode", TELEGRAM_PARSE_MODE.to_string()));
    }
    values
}

fn send_message_with_inline_keyboard_values(
    chat_id: i64,
    text: &str,
    reply_to_message_id: i64,
    buttons: &[InlineKeyboardButton],
    parse_markdown: bool,
) -> Result<Vec<(&'static str, String)>, String> {
    let mut values = send_message_values(chat_id, text, reply_to_message_id, None, parse_markdown);
    values.push((
        "reply_markup",
        serde_json::to_string(&serde_json::json!({
            "inline_keyboard": buttons
                .iter()
                .map(|button| vec![button])
                .collect::<Vec<_>>()
        }))
        .map_err(|err| err.to_string())?,
    ));
    Ok(values)
}

fn send_message_values(
    chat_id: i64,
    text: &str,
    reply_to_message_id: i64,
    message_effect_id: Option<&str>,
    parse_markdown: bool,
) -> Vec<(&'static str, String)> {
    let mut values = vec![
        ("chat_id", chat_id.to_string()),
        ("text", text.to_string()),
        ("disable_web_page_preview", "true".to_string()),
    ];
    if parse_markdown {
        values.push(("parse_mode", TELEGRAM_PARSE_MODE.to_string()));
    }
    if reply_to_message_id > 0 {
        values.push(("reply_to_message_id", reply_to_message_id.to_string()));
        values.push(("allow_sending_without_reply", "true".to_string()));
    }
    if let Some(effect_id) = message_effect_id.filter(|value| !value.trim().is_empty()) {
        values.push(("message_effect_id", effect_id.to_string()));
    }
    values
}

fn set_message_reaction_values(
    chat_id: i64,
    message_id: i64,
    emoji: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("chat_id", chat_id.to_string()),
        ("message_id", message_id.to_string()),
        (
            "reaction",
            serde_json::json!([{"type":"emoji","emoji":emoji}]).to_string(),
        ),
    ]
}

fn should_retry_plain_text(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("can't parse entities")
        || lower.contains("can't find end")
        || lower.contains("entity")
        || lower.contains("parse_mode")
}

pub fn supported_bot_commands() -> Vec<BotCommand> {
    DIRECTIVE_SPECS
        .iter()
        .map(|spec| BotCommand {
            command: spec.command().to_string(),
            description: spec.bot_description(),
        })
        .collect()
}

pub fn command_scope_targets(chat_ids: &[i64]) -> Vec<CommandScopeTarget> {
    let mut targets = vec![
        target("default", "default", None, false),
        target("all_private_chats", "all_private_chats", None, false),
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

fn redact_file_token(file_base_url: &str, value: &str) -> String {
    value.replace(file_base_url, "https://api.telegram.org/file/bot<redacted>")
}

#[cfg(test)]
fn file_base_url_for(base_url: &str) -> String {
    if let Some((prefix, token)) = base_url.rsplit_once("/bot") {
        return format!("{prefix}/file/bot{token}");
    }
    format!("{}/file", base_url.trim_end_matches('/'))
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
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

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
                "status", "log", "new", "restart", "update", "model", "resume", "rename", "list",
                "stop",
            ]
        );
        assert!(!names.contains(&"config"));
        assert!(!names.contains(&"help"));
        assert!(!names.contains(&"commands"));
        let expected_names: Vec<_> = DIRECTIVE_SPECS.iter().map(|spec| spec.command()).collect();
        let expected_descriptions: Vec<_> = DIRECTIVE_SPECS
            .iter()
            .map(|spec| spec.bot_description())
            .collect();
        let descriptions: Vec<_> = commands
            .iter()
            .map(|command| command.description.clone())
            .collect();

        assert_eq!(names, expected_names);
        assert_eq!(descriptions, expected_descriptions);
    }

    #[test]
    fn command_scope_targets_set_only_allowed_chat_scopes() {
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
                ("default", "default", false),
                ("all_private_chats", "all_private_chats", false),
                ("all_group_chats", "all_group_chats", false),
                ("all_chat_administrators", "all_chat_administrators", false),
                ("chat:42", "chat", true),
            ]
        );
        assert_eq!(targets[4].scope.chat_id, Some(42));
    }

    #[test]
    fn send_message_values_include_message_effect_id() {
        let values = send_message_values(42, "done", 7, Some("5107584321108051014"), true);

        assert!(values.contains(&("message_effect_id", "5107584321108051014".to_string())));
        assert!(values.contains(&("reply_to_message_id", "7".to_string())));
        assert!(values.contains(&("parse_mode", TELEGRAM_PARSE_MODE.to_string())));
    }

    #[test]
    fn set_message_reaction_values_encode_thumbsup() {
        let values = set_message_reaction_values(42, 7, "👍");
        let reaction = values
            .iter()
            .find_map(|(key, value)| (*key == "reaction").then_some(value))
            .unwrap();

        assert!(values.contains(&("chat_id", "42".to_string())));
        assert!(values.contains(&("message_id", "7".to_string())));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(reaction).unwrap(),
            serde_json::json!([{"type":"emoji","emoji":"👍"}])
        );
    }

    #[test]
    fn send_message_values_can_disable_markdown_parse_mode() {
        let values = send_message_values(42, "*done*", 0, None, false);

        assert!(!values.iter().any(|(key, _)| *key == "parse_mode"));
    }

    #[test]
    fn markdown_parse_errors_retry_as_plain_text() {
        assert!(should_retry_plain_text("Bad Request: can't parse entities"));
        assert!(!should_retry_plain_text("Network Error"));
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

    #[test]
    fn new_builds_default_telegram_base_url() {
        let client = TelegramClient::new("secret");

        assert_eq!(client.base_url, "https://api.telegram.org/botsecret");
        assert_eq!(
            client.file_base_url,
            "https://api.telegram.org/file/botsecret"
        );
    }

    #[test]
    fn get_updates_sends_expected_queries() {
        let server = TestServer::new(vec![
            json_response(r#"{"ok":true,"result":[]}"#),
            json_response(r#"{"ok":true,"result":[{"update_id":7,"message":null}]}"#),
        ]);
        let client = server.client();

        assert!(client.get_updates(0, 50).unwrap().is_empty());
        assert_eq!(client.get_updates(12, 5).unwrap()[0].update_id, 7);

        let first = server.request();
        let second = server.request();
        assert_eq!(first.method, "GET");
        assert!(first.path.starts_with("/botsecret/getUpdates?"));
        assert!(first.path.contains("timeout=50"));
        assert!(first.path.contains("allowed_updates="));
        assert!(!first.path.contains("offset="));
        assert!(second.path.contains("timeout=5"));
        assert!(second.path.contains("offset=12"));
    }

    #[test]
    fn download_file_resolves_telegram_file_path_and_writes_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let server = TestServer::new(vec![
            json_response(r#"{"ok":true,"result":{"file_id":"file-1","file_path":"docs/report.txt"}}"#),
            binary_response("downloaded bytes"),
        ]);
        let client = server.client();
        let target = dir.path().join("report.txt");

        client.download_file("file-1", &target).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "downloaded bytes");
        let get_file = server.request();
        let download = server.request();
        assert_eq!(get_file.path, "/botsecret/getFile");
        assert!(get_file.body.contains("file_id=file-1"));
        assert_eq!(download.method, "GET");
        assert_eq!(download.path, "/file/botsecret/docs/report.txt");
    }

    #[test]
    fn send_message_retries_plain_text_after_markdown_parse_error() {
        let server = TestServer::new(vec![
            json_response(r#"{"ok":false,"description":"Bad Request: can't parse entities"}"#),
            json_response(r#"{"ok":true,"result":{}}"#),
        ]);
        let client = server.client();

        client.send_message(42, "*broken", 7).unwrap();

        let first = server.request();
        let second = server.request();
        assert_eq!(first.path, "/botsecret/sendMessage");
        assert!(first.body.contains("chat_id=42"));
        assert!(first.body.contains("text=*broken") || first.body.contains("text=%2Abroken"));
        assert!(first.body.contains("reply_to_message_id=7"));
        assert!(first.body.contains("allow_sending_without_reply=true"));
        assert!(first.body.contains("parse_mode=Markdown"));
        assert!(!second.body.contains("parse_mode"));
    }

    #[test]
    fn send_message_redacts_private_data_before_posting() {
        let server = TestServer::new(vec![json_response(r#"{"ok":true,"result":{}}"#)]);
        let client = server.client();

        client
            .send_message(42, "OPENAI_API_KEY=sk-test-secret-value", 7)
            .unwrap();

        let request = server.request();
        assert!(!request.body.contains("sk-test-secret-value"));
        assert!(request.body.contains("redacted"));
    }

    #[test]
    fn send_message_returning_and_with_effect_decode_messages() {
        let server = TestServer::new(vec![
            json_response(message_json(55, None).as_str()),
            json_response(r#"{"ok":false,"description":"entity parse failed"}"#),
            json_response(message_json(56, Some("effect-1")).as_str()),
        ]);
        let client = server.client();

        assert_eq!(client.send_message_returning(42, "hello", 0).unwrap(), 55);
        let message = client
            .send_message_with_effect(42, "done", 7, "effect-1")
            .unwrap();

        assert_eq!(message.message_id, 56);
        assert_eq!(message.effect_id.as_deref(), Some("effect-1"));
        assert_eq!(server.request().path, "/botsecret/sendMessage");
        let retry_first = server.request();
        let retry_second = server.request();
        assert!(retry_first.body.contains("message_effect_id=effect-1"));
        assert!(retry_first.body.contains("parse_mode=Markdown"));
        assert!(retry_second.body.contains("message_effect_id=effect-1"));
        assert!(!retry_second.body.contains("parse_mode"));
    }

    #[test]
    fn message_mutation_methods_post_expected_forms() {
        let server = TestServer::new(vec![
            json_response(r#"{"ok":true,"result":true}"#),
            json_response(r#"{"ok":true,"result":true}"#),
            json_response(r#"{"ok":false,"description":"can't find end of the entity"}"#),
            json_response(r#"{"ok":true,"result":{}}"#),
            json_response(r#"{"ok":true,"result":true}"#),
        ]);
        let client = server.client();

        client.delete_message(42, 9).unwrap();
        client.set_message_reaction(42, 7, "👍").unwrap();
        client.edit_message_text(42, 8, "*fixed").unwrap();
        client.send_chat_action(42, "typing").unwrap();

        let delete = server.request();
        let reaction = server.request();
        let edit_markdown = server.request();
        let edit_plain = server.request();
        let action = server.request();
        assert_eq!(delete.path, "/botsecret/deleteMessage");
        assert!(delete.body.contains("message_id=9"));
        assert_eq!(reaction.path, "/botsecret/setMessageReaction");
        assert!(reaction.body.contains("reaction="));
        assert_eq!(edit_markdown.path, "/botsecret/editMessageText");
        assert!(edit_markdown.body.contains("parse_mode=Markdown"));
        assert!(!edit_plain.body.contains("parse_mode"));
        assert_eq!(action.path, "/botsecret/sendChatAction");
        assert!(action.body.contains("action=typing"));
    }

    #[test]
    fn sync_my_commands_deletes_global_scopes_and_sets_only_allowed_chat_scopes() {
        let responses = (0..18)
            .map(|_| json_response(r#"{"ok":true,"result":true}"#))
            .collect();
        let server = TestServer::new(responses);
        let client = server.client();

        client.sync_my_commands(&[42]).unwrap();

        let requests: Vec<_> = (0..18).map(|_| server.request()).collect();
        let deletes = requests
            .iter()
            .filter(|request| request.path == "/botsecret/deleteMyCommands")
            .count();
        let sets = requests
            .iter()
            .filter(|request| request.path == "/botsecret/setMyCommands")
            .count();
        assert_eq!(deletes, 15);
        assert_eq!(sets, 3);
        assert!(requests
            .iter()
            .any(|request| request.body.contains("language_code=en")));
        assert!(requests
            .iter()
            .any(|request| request.body.contains("commands=")));
    }

    #[test]
    fn telegram_errors_report_decode_missing_result_api_and_transport_failures() {
        let server = TestServer::new(vec![
            http_response("not json"),
            json_response(r#"{"ok":true}"#),
            json_response(r#"{"ok":false}"#),
        ]);
        let client = server.client();

        assert!(client
            .send_chat_action(42, "typing")
            .unwrap_err()
            .contains("decode telegram sendChatAction response"));
        assert_eq!(
            client.send_chat_action(42, "typing").unwrap_err(),
            "telegram sendChatAction returned no result"
        );
        assert_eq!(
            client.send_chat_action(42, "typing").unwrap_err(),
            "telegram sendChatAction failed"
        );

        let missing = TelegramClient {
            base_url: "http://127.0.0.1:1/botsecret".to_string(),
            file_base_url: "http://127.0.0.1:1/file/botsecret".to_string(),
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_millis(50))
                .build(),
        };
        let err = missing.send_chat_action(42, "typing").unwrap_err();
        assert!(err.contains("telegram sendChatAction request failed"));
        assert!(!err.contains("botsecret"));
    }

    #[test]
    fn send_message_methods_return_non_retry_errors() {
        let server = TestServer::new(vec![
            json_response(r#"{"ok":false,"description":"network down"}"#),
            json_response(r#"{"ok":false,"description":"still down"}"#),
            json_response(r#"{"ok":false,"description":"effect down"}"#),
            json_response(r#"{"ok":false,"description":"edit down"}"#),
        ]);
        let client = server.client();

        assert_eq!(
            client.send_message(42, "hello", 0).unwrap_err(),
            "network down"
        );
        assert_eq!(
            client.send_message_returning(42, "hello", 0).unwrap_err(),
            "still down"
        );
        assert_eq!(
            client
                .send_message_with_effect(42, "hello", 0, "effect")
                .unwrap_err(),
            "effect down"
        );
        assert_eq!(
            client.edit_message_text(42, 7, "hello").unwrap_err(),
            "edit down"
        );
    }

    #[test]
    fn send_message_returning_retries_plain_text_after_parse_error() {
        let server = TestServer::new(vec![
            json_response(r#"{"ok":false,"description":"parse_mode error"}"#),
            json_response(message_json(99, None).as_str()),
        ]);
        let client = server.client();

        assert_eq!(client.send_message_returning(42, "*hello", 7).unwrap(), 99);

        let markdown = server.request();
        let plain = server.request();
        assert!(markdown.body.contains("parse_mode=Markdown"));
        assert!(!plain.body.contains("parse_mode"));
    }

    #[test]
    fn non_success_json_errors_include_telegram_description() {
        let server = TestServer::new(vec![status_response(
            400,
            r#"{"ok":false,"description":"Bad Request: can't parse entities"}"#,
        )]);
        let client = server.client();

        let err = client.send_chat_action(42, "typing").unwrap_err();

        assert_eq!(err, "Bad Request: can't parse entities");
    }

    #[derive(Debug)]
    struct RecordedRequest {
        method: String,
        path: String,
        body: String,
    }

    struct TestServer {
        base_url: String,
        requests: Receiver<RecordedRequest>,
        _handle: JoinHandle<()>,
    }

    impl TestServer {
        fn new(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}/botsecret", listener.local_addr().unwrap());
            let (tx, requests) = mpsc::channel();
            let handle = thread::spawn(move || {
                for response in responses {
                    let (stream, _) = listener.accept().unwrap();
                    let request = read_request(&stream);
                    tx.send(request).unwrap();
                    write_response(stream, &response);
                }
            });
            Self {
                base_url,
                requests,
                _handle: handle,
            }
        }

        fn client(&self) -> TelegramClient {
            TelegramClient {
                base_url: self.base_url.clone(),
                file_base_url: file_base_url_for(&self.base_url),
                agent: ureq::AgentBuilder::new()
                    .timeout(std::time::Duration::from_secs(5))
                    .build(),
            }
        }

        fn request(&self) -> RecordedRequest {
            self.requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        }
    }

    fn read_request(stream: &TcpStream) -> RecordedRequest {
        let mut reader = BufReader::new(stream);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        let mut parts = first.split_whitespace();
        RecordedRequest {
            method: parts.next().unwrap_or_default().to_string(),
            path: parts.next().unwrap_or_default().to_string(),
            body: String::from_utf8(body).unwrap(),
        }
    }

    fn write_response(mut stream: TcpStream, response: &str) {
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn json_response(body: &str) -> String {
        http_response(body)
    }

    fn status_response(status: u16, body: &str) -> String {
        format!(
            "HTTP/1.1 {status} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn http_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn binary_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn message_json(message_id: i64, effect_id: Option<&str>) -> String {
        let effect = effect_id
            .map(|id| format!(r#","effect_id":"{id}""#))
            .unwrap_or_default();
        format!(
            r#"{{"ok":true,"result":{{"message_id":{message_id}{effect},"from":null,"chat":{{"id":42,"type":"private"}},"text":"sent","caption":""}}}}"#
        )
    }
}
