use crate::codex::{run_codex, run_codex_stream, CodexConfig, CodexRun, LIGHTWEIGHT_CODEX_MODEL};
use crate::commands::{directive_from_command, is_allowed, unknown_directive_message, Directive};
use crate::config::{Config, ProviderModel};
use crate::logs;
use crate::provider::Provider;
use crate::session::{SessionKey, SessionStore};
use crate::status::{format_status_message, status_sections};
use crate::telegram::{CallbackQuery, InlineKeyboardButton, Message, TelegramClient, Update};
use crate::text::{
    command_arg, is_ok_response, log_line_count, parse_command, redact_private_data, session_label,
    split_telegram_message, tail_log_text,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TYPING_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const POLL_CONFLICT_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const POLL_REQUEST_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const TELEGRAM_GET_UPDATES_CONFLICT_MARKER: &str = "terminated by other getUpdates request";
const GATEWAY_UPDATE_JOB_LABEL: &str = "ai.gateway.update";
const GATEWAY_UPDATE_PENDING_LOCK_TTL_SECS: u64 = 300;
const GATEWAY_UPDATE_SCRIPT: &str = r#"gateway_update_label="$1"
gateway_update_lock="$2"
gateway_update_root="$3"
gateway_update_log="${gateway_update_lock:h}/logs/update.log"
set -o pipefail
mkdir -p "${gateway_update_log:h}"
print -r -- "$$" > "$gateway_update_lock"
{
  print -r -- "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] starting gateway update"
  cd "$gateway_update_root" &&
    git pull &&
    brew update &&
    brew upgrade &&
    brew cleanup &&
    (curl -sSfL "https://raw.githubusercontent.com/foundry-rs/foundry/refs/heads/master/foundryup/foundryup" | bash) &&
    ./setup
  gateway_update_code=$?
  print -r -- "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] gateway update exited $gateway_update_code"
} >>"$gateway_update_log" 2>&1
rm -f "$gateway_update_lock"
/bin/launchctl remove "$gateway_update_label" >/dev/null 2>&1 || true
exit 0"#;

trait TelegramApi: Clone + Send + 'static {
    fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String>;
    fn sync_my_commands(&self, chat_ids: &[i64]) -> Result<(), String>;
    fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<(), String>;
    fn send_message_with_inline_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        buttons: &[InlineKeyboardButton],
    ) -> Result<(), String>;
    fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<(), String>;
    fn send_message_returning(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<i64, String>;
    fn send_message_with_effect(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        message_effect_id: &str,
    ) -> Result<Message, String>;
    fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), String>;
    fn set_message_reaction(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: &str,
    ) -> Result<(), String>;
    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), String>;
    fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String>;
    fn download_file(&self, file_id: &str, path: &Path) -> Result<(), String>;
}

impl TelegramApi for TelegramClient {
    fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String> {
        self.get_updates(offset, timeout_sec)
    }

    fn sync_my_commands(&self, chat_ids: &[i64]) -> Result<(), String> {
        self.sync_my_commands(chat_ids)
    }

    fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<(), String> {
        self.send_message(chat_id, text, reply_to_message_id)
    }
    fn send_message_with_inline_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        buttons: &[InlineKeyboardButton],
    ) -> Result<(), String> {
        self.send_message_with_inline_keyboard(chat_id, text, reply_to_message_id, buttons)
    }
    fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<(), String> {
        self.answer_callback_query(callback_query_id, text)
    }

    fn send_message_returning(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
    ) -> Result<i64, String> {
        self.send_message_returning(chat_id, text, reply_to_message_id)
    }

    fn send_message_with_effect(
        &self,
        chat_id: i64,
        text: &str,
        reply_to_message_id: i64,
        message_effect_id: &str,
    ) -> Result<Message, String> {
        self.send_message_with_effect(chat_id, text, reply_to_message_id, message_effect_id)
    }

    fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), String> {
        self.delete_message(chat_id, message_id)
    }

    fn set_message_reaction(
        &self,
        chat_id: i64,
        message_id: i64,
        emoji: &str,
    ) -> Result<(), String> {
        self.set_message_reaction(chat_id, message_id, emoji)
    }

    fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<(), String> {
        self.edit_message_text(chat_id, message_id, text)
    }

    fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String> {
        self.send_chat_action(chat_id, action)
    }

    fn download_file(&self, file_id: &str, path: &Path) -> Result<(), String> {
        self.download_file(file_id, path)
    }
}

#[derive(Debug)]
struct Job {
    bot_token: String,
    chat_id: i64,
    thread_id: Option<i64>,
    reply_to_message_id: i64,
    prompt: String,
    _attachment_dir: Option<tempfile::TempDir>,
    image_paths: Vec<PathBuf>,
    file_paths: Vec<PathBuf>,
    provider_model: ProviderModel,
    cancel_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttachmentKind {
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentSpec {
    file_id: String,
    file_name: String,
    kind: AttachmentKind,
}

#[derive(Debug, Default)]
struct DownloadedAttachments {
    dir: Option<tempfile::TempDir>,
    image_paths: Vec<PathBuf>,
    file_paths: Vec<PathBuf>,
}

type RuntimeSelections = Arc<Mutex<HashMap<SessionKey, ProviderModel>>>;
type CancellationState = Arc<Mutex<HashMap<SessionKey, CancellationEntry>>>;

#[derive(Debug, Clone)]
struct CancellationEntry {
    epoch: u64,
    active: Vec<Arc<AtomicBool>>,
}

struct TypingLoop {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Debug, PartialEq, Eq)]
enum FinalDelivery {
    Reaction,
    Message(String),
}

impl Drop for TypingLoop {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn run(cfg: Config) -> Result<(), String> {
    if cfg.telegram_bots.len() <= 1 {
        let tg = TelegramClient::new(&cfg.bot_token);
        return run_with_client(cfg, tg);
    }

    let (tx, rx) = mpsc::channel();
    for bot in cfg.telegram_bots.clone() {
        let tx = tx.clone();
        let mut bot_cfg = cfg.clone();
        bot_cfg.bot_token = bot.bot_token;
        bot_cfg.telegram_chat_ids = bot.chat_ids;
        bot_cfg.offset_file = bot.offset_file;
        bot_cfg.telegram_bots = vec![crate::config::TelegramBotConfig {
            bot_token: bot_cfg.bot_token.clone(),
            chat_ids: bot_cfg.telegram_chat_ids.clone(),
            offset_file: bot_cfg.offset_file.clone(),
        }];
        thread::spawn(move || {
            let tg = TelegramClient::new(&bot_cfg.bot_token);
            let result = run_with_client(bot_cfg, tg);
            let _ = tx.send(result);
        });
    }
    drop(tx);
    rx.recv()
        .map_err(|err| format!("telegram bot workers exited: {err}"))?
}

fn run_with_client<T: TelegramApi>(cfg: Config, tg: T) -> Result<(), String> {
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    fs::create_dir_all(&cfg.chat_state_dir)
        .map_err(|err| format!("create chat state dir: {err}"))?;

    if let Err(err) = tg.sync_my_commands(&cfg.telegram_chat_ids) {
        logs::warn(format_args!(
            "telegram command sync failed; continuing without command refresh: {err}"
        ));
    }
    let default_model = cfg.default_provider_model().clone();
    let store = SessionStore::new_with_provider(
        cfg.chat_state_dir.clone(),
        default_model.model.clone(),
        default_model.provider,
    );
    let codex = CodexConfig::from(&cfg);
    let sections = status_sections(&cfg, &codex);
    for chat_id in &cfg.telegram_chat_ids {
        let key = SessionKey::Chat {
            chat_id: *chat_id,
            thread_id: None,
        };
        if let Err(err) = auto_rename_startup_session(&cfg, &codex, &tg, &store, *chat_id, &key) {
            logs::warn(format_args!(
                "telegram startup session rename failed for chat {chat_id}: {err}"
            ));
        }
        let state = store.load(&key);
        if let Err(err) = send_long_message(
            &tg,
            *chat_id,
            &format_status_message(&state, &sections.codex, &sections.git, &sections.fetch),
            0,
        ) {
            logs::warn(format_args!(
                "telegram startup status send failed for chat {chat_id}: {err}"
            ));
        }
    }

    let (tx, rx) = mpsc::sync_channel::<Job>(cfg.queue_depth);
    let cancellations = CancellationState::default();
    let worker_cfg = cfg.clone();
    let worker_cancellations = cancellations.clone();
    let _worker = thread::spawn(move || worker_loop(worker_cfg, rx, worker_cancellations));
    let selections = RuntimeSelections::default();

    let mut offset = read_offset(&cfg.offset_file);
    if offset == 0 {
        if let Ok(updates) = tg.get_updates(0, 0) {
            offset = skip_offset(&updates);
            if offset > 0 {
                write_offset(&cfg.offset_file, offset)?;
            }
        }
    }
    loop {
        let updates = match tg.get_updates(offset, cfg.poll_timeout_sec) {
            Ok(updates) => updates,
            Err(err) if is_get_updates_conflict(&err) => {
                logs::warn(format_args!(
                    "telegram getUpdates conflict; another bot instance is polling this token; retrying in {}s: {err}",
                    POLL_CONFLICT_RETRY_INTERVAL.as_secs()
                ));
                sleep_after_get_updates_conflict();
                continue;
            }
            Err(err) if is_transient_get_updates_error(&err) => {
                logs::warn(format_args!(
                    "telegram getUpdates request failed; retrying in {}s: {err}",
                    POLL_REQUEST_RETRY_INTERVAL.as_secs()
                ));
                sleep_after_get_updates_request_failure();
                continue;
            }
            Err(err) => return Err(err),
        };
        for update in updates {
            offset = advance_offset(offset, update.update_id);
            write_offset(&cfg.offset_file, offset)?;
            if let Some(message) = update.message {
                if let Err(err) =
                    handle_message(&cfg, &tg, &store, &selections, &cancellations, &tx, message)
                {
                    logs::warn(format_args!("message handler failed: {err}"));
                }
            }
            if let Some(callback_query) = update.callback_query {
                if let Err(err) =
                    handle_callback_query(&cfg, &tg, &store, &selections, callback_query)
                {
                    logs::warn(format_args!("callback handler failed: {err}"));
                }
            }
        }
    }
}

fn is_get_updates_conflict(err: &str) -> bool {
    err.starts_with("Conflict:") && err.contains(TELEGRAM_GET_UPDATES_CONFLICT_MARKER)
}

fn is_transient_get_updates_error(err: &str) -> bool {
    err.starts_with("telegram getUpdates request failed:")
}

fn sleep_after_get_updates_conflict() {
    #[cfg(not(test))]
    thread::sleep(POLL_CONFLICT_RETRY_INTERVAL);
}

fn sleep_after_get_updates_request_failure() {
    #[cfg(not(test))]
    thread::sleep(POLL_REQUEST_RETRY_INTERVAL);
}

pub fn read_offset(path: &Path) -> i64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

pub fn write_offset(path: &Path, offset: i64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("offset.tmp");
    fs::write(&tmp, format!("{offset}\n")).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

pub fn message_text(text: &str, caption: &str) -> Result<String, String> {
    let text = text.trim();
    if !text.is_empty() {
        return Ok(text.to_string());
    }
    let caption = caption.trim();
    if !caption.is_empty() {
        return Ok(caption.to_string());
    }
    Err("📝 Text messages only.".to_string())
}

fn message_prompt_text(msg: &Message, attachments: &[AttachmentSpec]) -> Result<String, String> {
    match message_text(&msg.text, &msg.caption) {
        Ok(text) => Ok(text),
        Err(err) if attachments.is_empty() => Err(err),
        Err(_) => Ok(default_attachment_prompt(attachments)),
    }
}

fn message_attachment_specs(msg: &Message) -> Vec<AttachmentSpec> {
    let mut attachments = Vec::new();
    if let Some(photo) = msg.photo.iter().max_by_key(|photo| {
        (
            photo.file_size.unwrap_or(0),
            u64::from(photo.width) * u64::from(photo.height),
        )
    }) {
        attachments.push(AttachmentSpec {
            file_id: photo.file_id.clone(),
            file_name: format!("photo-{}.jpg", msg.message_id),
            kind: AttachmentKind::Image,
        });
    }
    if let Some(document) = msg.document.as_ref() {
        let kind = if document
            .mime_type
            .to_ascii_lowercase()
            .starts_with("image/")
        {
            AttachmentKind::Image
        } else {
            AttachmentKind::File
        };
        let fallback = match kind {
            AttachmentKind::Image => format!("image-{}", msg.message_id),
            AttachmentKind::File => format!("file-{}", msg.message_id),
        };
        attachments.push(AttachmentSpec {
            file_id: document.file_id.clone(),
            file_name: document_file_name(&document.file_name, &fallback),
            kind,
        });
    }
    attachments
}

fn document_file_name(name: &str, fallback: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        fallback.to_string()
    } else {
        name.to_string()
    }
}

fn default_attachment_prompt(attachments: &[AttachmentSpec]) -> String {
    let has_image = attachments
        .iter()
        .any(|attachment| attachment.kind == AttachmentKind::Image);
    let has_file = attachments
        .iter()
        .any(|attachment| attachment.kind == AttachmentKind::File);
    match (has_image, has_file) {
        (true, true) => "Please review the attached Telegram images and files.".to_string(),
        (true, false) => "Please review the attached Telegram image.".to_string(),
        (false, true) => "Please review the attached Telegram file.".to_string(),
        (false, false) => "Please review this Telegram message.".to_string(),
    }
}

fn download_message_attachments(
    cfg: &Config,
    tg: &impl TelegramApi,
    attachments: &[AttachmentSpec],
) -> Result<DownloadedAttachments, String> {
    if attachments.is_empty() {
        return Ok(DownloadedAttachments::default());
    }
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    let dir = tempfile::Builder::new()
        .prefix("attach-")
        .tempdir_in(&cfg.state_dir)
        .map_err(|err| format!("create attachment dir: {err}"))?;
    let mut image_paths = Vec::new();
    let mut file_paths = Vec::new();
    for (index, attachment) in attachments.iter().enumerate() {
        let path = dir
            .path()
            .join(local_attachment_file_name(index, &attachment.file_name));
        tg.download_file(&attachment.file_id, &path)?;
        match attachment.kind {
            AttachmentKind::Image => image_paths.push(path),
            AttachmentKind::File => file_paths.push(path),
        }
    }
    Ok(DownloadedAttachments {
        dir: Some(dir),
        image_paths,
        file_paths,
    })
}

fn local_attachment_file_name(index: usize, name: &str) -> String {
    let name = name
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("attachment");
    let cleaned = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let cleaned = cleaned.trim_matches('_');
    let cleaned = if cleaned.is_empty() {
        "attachment"
    } else {
        cleaned
    };
    format!("{:02}-{cleaned}", index + 1)
}

fn prompt_with_file_attachments(prompt: &str, file_paths: &[PathBuf]) -> String {
    if file_paths.is_empty() {
        return prompt.to_string();
    }
    let files = file_paths
        .iter()
        .map(|path| format!("- {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}\n\nTelegram file attachments:\n{files}", prompt.trim())
}

fn prompt_with_reply_context(msg: &Message, text: &str) -> String {
    let Some(reply) = msg.reply_to_message.as_deref() else {
        return text.to_string();
    };
    let Ok(reply_text) = message_text(&reply.text, &reply.caption) else {
        return text.to_string();
    };
    format!(
        "Telegram reply context:\n{}\n\nUser message:\n{}",
        reply_text.trim(),
        text.trim()
    )
}

fn advance_offset(current: i64, update_id: i64) -> i64 {
    current.max(update_id + 1)
}

fn skip_offset(updates: &[Update]) -> i64 {
    updates
        .iter()
        .map(|update| update.update_id + 1)
        .max()
        .unwrap_or(0)
}

fn handle_message(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    cancellations: &CancellationState,
    tx: &mpsc::SyncSender<Job>,
    msg: Message,
) -> Result<(), String> {
    if !is_allowed_private_message(&cfg.telegram_chat_ids, &msg) {
        return Ok(());
    }
    let attachment_specs = message_attachment_specs(&msg);
    let text = match message_prompt_text(&msg, &attachment_specs) {
        Ok(text) => text,
        Err(err) => {
            tg.send_message(msg.chat.id, &err, msg.message_id)?;
            return Ok(());
        }
    };

    if let Some(command) = parse_command(&text) {
        return handle_command(
            cfg,
            tg,
            store,
            selections,
            cancellations,
            &msg,
            &text,
            &command,
        );
    }
    let key = SessionKey::Chat {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
    };
    let attachments = match download_message_attachments(cfg, tg, &attachment_specs) {
        Ok(attachments) => attachments,
        Err(err) => {
            tg.send_message(
                msg.chat.id,
                &format!("⚠️ Failed to download Telegram attachment: {err}"),
                msg.message_id,
            )?;
            return Ok(());
        }
    };
    let prompt = prompt_with_file_attachments(
        &prompt_with_reply_context(&msg, &text),
        &attachments.file_paths,
    );

    let queued = tx.try_send(Job {
        bot_token: cfg.bot_token.clone(),
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
        reply_to_message_id: msg.message_id,
        prompt,
        _attachment_dir: attachments.dir,
        image_paths: attachments.image_paths,
        file_paths: attachments.file_paths,
        provider_model: selected_provider_model(cfg, selections, &key),
        cancel_epoch: cancellation_epoch(cancellations, &key),
    });
    if queued.is_err() {
        tg.send_message(
            msg.chat.id,
            "🚦 Codex queue is full. Try again after the current requests finish.",
            msg.message_id,
        )?;
    } else {
        let _ = tg.send_chat_action(msg.chat.id, "typing");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    cancellations: &CancellationState,
    msg: &Message,
    text: &str,
    command: &str,
) -> Result<(), String> {
    let codex = CodexConfig::from(cfg);
    handle_command_with_codex(
        cfg,
        &codex,
        tg,
        store,
        selections,
        cancellations,
        msg,
        text,
        command,
    )
}

#[allow(clippy::too_many_arguments)]
fn handle_command_with_codex(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    cancellations: &CancellationState,
    msg: &Message,
    text: &str,
    command: &str,
) -> Result<(), String> {
    let key = SessionKey::Chat {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
    };
    match directive_from_command(command) {
        Some(Directive::Log) => handle_log_command(cfg, tg, msg, text),
        Some(Directive::New) => handle_new_command(cfg, codex, tg, store, selections, msg, &key),
        Some(Directive::Restart) => {
            tg.send_message(msg.chat.id, "🔄 Restarting gateway.", msg.message_id)?;
            restart_gateway(&cfg.launchd_target);
            Ok(())
        }
        Some(Directive::Update) => handle_update_command(cfg, tg, msg),
        Some(Directive::Model) => handle_model_command(cfg, tg, store, selections, msg, text, &key),
        Some(Directive::Resume) => handle_resume_command(tg, store, selections, msg, text, &key),
        Some(Directive::Rename) => handle_rename_command(cfg, codex, tg, store, msg, text, &key),
        Some(Directive::List) => {
            send_long_message(tg, msg.chat.id, &store.list(&key), msg.message_id)
        }
        Some(Directive::Stop) => handle_stop_command(tg, cancellations, msg, &key),
        Some(Directive::Status) => {
            handle_status_command(cfg, codex, tg, store, selections, msg, &key)
        }
        None => tg.send_message(msg.chat.id, &unknown_directive_message(), msg.message_id),
    }
}

fn handle_stop_command(
    tg: &impl TelegramApi,
    cancellations: &CancellationState,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    let active = cancel_key(cancellations, key);
    let detail = if active == 0 {
        "No active Codex run was found; queued work for this chat was cancelled."
    } else {
        "Active Codex work was cancelled; queued work for this chat was skipped."
    };
    tg.send_message(msg.chat.id, &format!("🛑 {detail}"), msg.message_id)
}

fn handle_log_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    msg: &Message,
    text: &str,
) -> Result<(), String> {
    let lines = log_line_count(text);
    let body = fs::read_to_string(&cfg.gateway_log_file)
        .map(|log_text| tail_log_text(&log_text, lines))
        .unwrap_or_else(|_| "📭 No gateway log available.".to_string());
    send_long_message(tg, msg.chat.id, &body, msg.message_id)
}

fn handle_new_command(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    let state = store.load(key);
    if current_session_is_unnamed(&state)
        && !auto_rename_current_session(cfg, codex, tg, store, msg, key)?
    {
        return Ok(());
    }
    match store.reset(key) {
        Ok(state) => {
            clear_selection(selections, key);
            tg.send_message(
                msg.chat.id,
                &format!("🆕 New session ready. 🤖 Model: {}", state.model),
                msg.message_id,
            )
        }
        Err(err) => tg.send_message(
            msg.chat.id,
            &format!("⚠️ Failed to reset session: {err}"),
            msg.message_id,
        ),
    }
}

fn current_session_is_unnamed(state: &crate::session::ChatSession) -> bool {
    let Some(session_id) = state.session_id.as_deref() else {
        return false;
    };
    match state
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| session.name.as_deref())
    {
        Some(name) => name.trim().is_empty(),
        None => true,
    }
}

fn handle_model_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    text: &str,
    key: &SessionKey,
) -> Result<(), String> {
    let arg = command_arg(text);
    if arg.is_empty() {
        return tg.send_message_with_inline_keyboard(
            msg.chat.id,
            "🤖 Select model:",
            msg.message_id,
            &model_buttons(cfg),
        );
    }
    let Ok(index) = arg.parse::<usize>() else {
        return tg.send_message(
            msg.chat.id,
            &format!(
                "🧭 Usage: /model or /model 0..{}",
                cfg.models.len().saturating_sub(1)
            ),
            msg.message_id,
        );
    };
    select_model_slot(
        cfg,
        tg,
        store,
        selections,
        ModelSelectionContext {
            chat_id: msg.chat.id,
            reply_to_message_id: msg.message_id,
            key,
        },
        index,
    )
}

struct ModelSelectionContext<'a> {
    chat_id: i64,
    reply_to_message_id: i64,
    key: &'a SessionKey,
}

fn select_model_slot(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    context: ModelSelectionContext<'_>,
    index: usize,
) -> Result<(), String> {
    let Some(choice) = cfg.provider_model_at(index).cloned() else {
        return tg.send_message(
            context.chat_id,
            &format!(
                "🧭 Unknown model slot {index}. Use /model and choose one of 0..{}.",
                cfg.models.len().saturating_sub(1)
            ),
            context.reply_to_message_id,
        );
    };
    set_selection(selections, context.key, choice.clone());
    store.reset(context.key)?;
    tg.send_message(
        context.chat_id,
        &format!(
            "🤖 Selected {}\n🧵 Session: none",
            provider_model_label(&choice)
        ),
        context.reply_to_message_id,
    )
}

fn model_buttons(cfg: &Config) -> Vec<InlineKeyboardButton> {
    cfg.models
        .iter()
        .enumerate()
        .map(|(index, choice)| InlineKeyboardButton {
            text: provider_model_label(choice),
            callback_data: format!("model:{index}"),
        })
        .collect()
}

fn provider_model_label(choice: &ProviderModel) -> String {
    format!("{}: {}", choice.provider.label(), choice.model)
}

fn selected_provider_model(
    cfg: &Config,
    selections: &RuntimeSelections,
    key: &SessionKey,
) -> ProviderModel {
    selections
        .lock()
        .unwrap()
        .get(key)
        .cloned()
        .unwrap_or_else(|| cfg.default_provider_model().clone())
}

fn set_selection(selections: &RuntimeSelections, key: &SessionKey, choice: ProviderModel) {
    selections.lock().unwrap().insert(key.clone(), choice);
}

fn clear_selection(selections: &RuntimeSelections, key: &SessionKey) {
    selections.lock().unwrap().remove(key);
}

fn handle_callback_query(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    query: CallbackQuery,
) -> Result<(), String> {
    let Some(message) = query.message.as_ref() else {
        return tg.answer_callback_query(&query.id, "Message unavailable.");
    };
    if !is_allowed_private_callback(&cfg.telegram_chat_ids, &query) {
        return Ok(());
    }
    let Some(raw_index) = query.data.strip_prefix("model:") else {
        return tg.answer_callback_query(&query.id, "Unknown action.");
    };
    let Ok(index) = raw_index.parse::<usize>() else {
        return tg.answer_callback_query(&query.id, "Unknown model slot.");
    };
    let key = SessionKey::Chat {
        chat_id: message.chat.id,
        thread_id: message.message_thread_id,
    };
    let Some(choice) = cfg.provider_model_at(index).cloned() else {
        return tg.answer_callback_query(&query.id, "Unknown model slot.");
    };
    set_selection(selections, &key, choice.clone());
    store.reset(&key)?;
    tg.answer_callback_query(
        &query.id,
        &format!("Selected {}", provider_model_label(&choice)),
    )?;
    tg.send_message(
        message.chat.id,
        &format!(
            "🤖 Selected {}\n🧵 Session: none",
            provider_model_label(&choice)
        ),
        message.message_id,
    )
}

fn is_allowed_private_message(telegram_chat_ids: &[i64], msg: &Message) -> bool {
    is_allowed_private_chat(
        telegram_chat_ids,
        msg.chat.id,
        &msg.chat.kind,
        msg.from.as_ref(),
    )
}

fn is_allowed_private_callback(telegram_chat_ids: &[i64], query: &CallbackQuery) -> bool {
    let Some(message) = query.message.as_ref() else {
        return false;
    };
    is_allowed_private_chat(
        telegram_chat_ids,
        message.chat.id,
        &message.chat.kind,
        Some(&query.from),
    )
}

fn is_allowed_private_chat(
    telegram_chat_ids: &[i64],
    chat_id: i64,
    chat_kind: &str,
    from: Option<&crate::telegram::User>,
) -> bool {
    is_allowed(telegram_chat_ids, chat_id)
        && chat_kind == "private"
        && from.is_some_and(|user| user.id == chat_id)
}

fn handle_resume_command(
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    text: &str,
    key: &SessionKey,
) -> Result<(), String> {
    let target = command_arg(text);
    if target.is_empty() || target == "0" {
        let body = store.list(key);
        return send_long_message(tg, msg.chat.id, &body, msg.message_id);
    }
    let result = target.parse::<usize>().map_or_else(
        |_| store.resume(key, &target),
        |index| store.resume_index(key, index),
    );
    match result {
        Ok(state) => {
            clear_selection(selections, key);
            send_resumed_session(tg, msg, &state)
        }
        Err(err) => tg.send_message(msg.chat.id, &err, msg.message_id),
    }
}

fn send_resumed_session(
    tg: &impl TelegramApi,
    msg: &Message,
    state: &crate::session::ChatSession,
) -> Result<(), String> {
    tg.send_message(
        msg.chat.id,
        &format!(
            "↩️ Resumed session {}\n🤖 Model: {}",
            session_label(state.session_id.as_deref().unwrap_or("")),
            state.model
        ),
        msg.message_id,
    )
}

fn handle_rename_command(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    text: &str,
    key: &SessionKey,
) -> Result<(), String> {
    let name = command_arg(text);
    if name.is_empty() {
        return handle_auto_rename_command(cfg, codex, tg, store, msg, key);
    }
    rename_session(tg, store, msg, key, &name)
}

fn handle_auto_rename_command(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    auto_rename_current_session(cfg, codex, tg, store, msg, key).map(|_| ())
}

fn auto_rename_current_session(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
) -> Result<bool, String> {
    auto_rename_session(cfg, codex, tg, store, msg.chat.id, msg.message_id, key)
}

fn auto_rename_startup_session(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    chat_id: i64,
    key: &SessionKey,
) -> Result<bool, String> {
    let state = store.load(key);
    if !current_session_is_unnamed(&state) {
        return Ok(false);
    }
    auto_rename_session(cfg, codex, tg, store, chat_id, 0, key)
}

fn auto_rename_session(
    cfg: &Config,
    codex: &CodexConfig,
    _tg: &impl TelegramApi,
    store: &SessionStore,
    chat_id: i64,
    _reply_to_message_id: i64,
    key: &SessionKey,
) -> Result<bool, String> {
    let state = store.load(key);
    if state.session_id.is_none() {
        return Ok(false);
    }
    let cfg = cfg.clone();
    let codex = codex.clone();
    let store = store.clone();
    let key = key.clone();
    thread::spawn(move || {
        if let Err(err) = auto_rename_session_in_background(&cfg, &codex, &store, &key, state) {
            logs::warn(format_args!(
                "telegram auto session rename failed for chat {chat_id}: {err}"
            ));
        }
    });
    Ok(true)
}

fn auto_rename_session_in_background(
    cfg: &Config,
    codex: &CodexConfig,
    store: &SessionStore,
    key: &SessionKey,
    state: crate::session::ChatSession,
) -> Result<(), String> {
    let session_id = state
        .session_id
        .as_deref()
        .ok_or_else(|| "no current session to rename".to_string())?;
    let previous_name = state
        .saved_session_name(session_id)
        .map(ToString::to_string);
    let output = match run_codex(
        codex,
        AUTO_RENAME_PROMPT,
        Some(session_id),
        Provider::Codex,
        LIGHTWEIGHT_CODEX_MODEL,
        cfg.codex_timeout,
        &cfg.state_dir,
    ) {
        Ok(output) => output,
        Err(err) => return Err(format!("run Codex title prompt: {err}")),
    };
    let Some(name) = auto_session_name(&output.final_text) else {
        return Err("Codex returned an invalid session name".to_string());
    };

    let target_session_id = match output.session_id.as_deref() {
        Some(output_session_id) => {
            if store.save_current_session(key, state.generation, output_session_id)? {
                output_session_id.to_string()
            } else {
                session_id.to_string()
            }
        }
        None => session_id.to_string(),
    };

    let expected_name = if target_session_id == session_id {
        previous_name.as_deref()
    } else {
        None
    };
    store.rename_session_if_name_unchanged(
        key,
        &target_session_id,
        expected_name,
        &name,
        &state.model,
        state.provider,
    )?;
    Ok(())
}

const AUTO_RENAME_PROMPT: &str = "Create a concise name for this session. Return only the name, with no quotes, prefixes, markdown, or explanation. Use a lowercase single-word name, or if multiple words are necessary, use a lowercase hyphenated name like session-name.";

fn auto_session_name(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| {
            let name = line
                .trim()
                .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*'))
                .trim();
            is_valid_session_name(name).then(|| name.to_string())
        })
        .next()
}

fn is_valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        })
}

fn rename_session(
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
    name: &str,
) -> Result<(), String> {
    let state = store.load(key);
    let store = store.clone();
    let key = key.clone();
    let name = name.to_string();
    let chat_id = msg.chat.id;
    thread::spawn(move || {
        if let Err(err) = rename_session_in_background(&store, &key, state, &name) {
            logs::warn(format_args!(
                "telegram explicit session rename failed for chat {chat_id}: {err}"
            ));
        }
    });
    react_ok(tg, msg.chat.id, msg.message_id)
}

fn rename_session_in_background(
    store: &SessionStore,
    key: &SessionKey,
    state: crate::session::ChatSession,
    name: &str,
) -> Result<(), String> {
    let session_id = state
        .session_id
        .as_deref()
        .ok_or_else(|| "no current session to rename".to_string())?;
    store.rename_session(key, session_id, name, &state.model, state.provider)?;
    Ok(())
}

fn react_ok(tg: &impl TelegramApi, chat_id: i64, message_id: i64) -> Result<(), String> {
    tg.set_message_reaction(chat_id, message_id, "👍")
}

fn handle_status_command(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    let mut state = store.load(key);
    if current_session_is_unnamed(&state)
        && !auto_rename_current_session(cfg, codex, tg, store, msg, key)?
    {
        return Ok(());
    }
    state = store.load(key);
    let sections = status_sections(cfg, codex);
    send_long_message(
        tg,
        msg.chat.id,
        &format_status_message(
            &state_with_provider_model(&state, &selected_provider_model(cfg, selections, key)),
            &sections.codex,
            &sections.git,
            &sections.fetch,
        ),
        msg.message_id,
    )
}

fn state_with_provider_model(
    state: &crate::session::ChatSession,
    choice: &ProviderModel,
) -> crate::session::ChatSession {
    let mut state = state.clone();
    state.provider = choice.provider;
    state.model = choice.model.clone();
    state
}

fn worker_loop(cfg: Config, rx: mpsc::Receiver<Job>, cancellations: CancellationState) {
    let default_model = cfg.default_provider_model().clone();
    let store = SessionStore::new_with_provider(
        cfg.chat_state_dir.clone(),
        default_model.model,
        default_model.provider,
    );
    for job in rx {
        let tg = TelegramClient::new(&job.bot_token);
        if is_job_cancelled(&cancellations, &job) {
            logs::info(format_args!(
                "job skipped after cancellation chat={} reply_to={}",
                job.chat_id, job.reply_to_message_id
            ));
            continue;
        }
        if let Err(err) = run_job(&cfg, &tg, &store, &cancellations, job) {
            logs::error(format_args!("job handler failed: {err}"));
        }
    }
}

fn run_job(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    cancellations: &CancellationState,
    job: Job,
) -> Result<(), String> {
    let codex = CodexConfig::from(cfg);
    run_job_with_codex(cfg, &codex, tg, store, cancellations, job)
}

fn run_job_with_codex(
    cfg: &Config,
    codex: &CodexConfig,
    tg: &impl TelegramApi,
    store: &SessionStore,
    cancellations: &CancellationState,
    job: Job,
) -> Result<(), String> {
    let started = Instant::now();
    let key = SessionKey::Chat {
        chat_id: job.chat_id,
        thread_id: job.thread_id,
    };
    let state = store.load(&key);
    logs::info(format_args!(
        "job start chat={} reply_to={} provider={} model={} session={} prompt_chars={} attachments={} timeout_secs={}",
        job.chat_id,
        job.reply_to_message_id,
        job.provider_model.provider.label(),
        job.provider_model.model,
        session_label(state.session_id.as_deref().unwrap_or("")),
        job.prompt.chars().count(),
        job.image_paths.len() + job.file_paths.len(),
        cfg.codex_timeout.as_secs()
    ));
    let stream_message_id =
        tg.send_message_returning(job.chat_id, "🫧 Thinking…", job.reply_to_message_id)?;
    let cancel = register_active_cancel(cancellations, &key);
    let mut streamed = String::new();
    let mut last_edit = Instant::now();
    let run_result = {
        let _typing = start_typing_loop(tg, job.chat_id);
        run_codex_stream(
            codex,
            CodexRun {
                prompt: &job.prompt,
                session_id: state.session_id.as_deref(),
                provider: job.provider_model.provider,
                model: &job.provider_model.model,
                image_paths: &job.image_paths,
                timeout: cfg.codex_timeout,
                state_dir: &cfg.state_dir,
                cancel: Some(cancel.clone()),
            },
            |chunk| {
                streamed.push_str(chunk);
                if last_edit.elapsed() >= Duration::from_millis(1200) {
                    let _ = tg.edit_message_text(
                        job.chat_id,
                        stream_message_id,
                        &stream_preview(&streamed),
                    );
                    last_edit = Instant::now();
                }
            },
        )
    };
    let output = match run_result {
        Ok(output) => output,
        Err(err) => {
            unregister_active_cancel(cancellations, &key, &cancel);
            logs::warn(format_args!(
                "job provider failed chat={} provider={} elapsed_ms={} error={}",
                job.chat_id,
                job.provider_model.provider.label(),
                started.elapsed().as_millis(),
                single_line(&err)
            ));
            if err == "codex cancelled" {
                let _ = tg.delete_message(job.chat_id, stream_message_id);
                tg.send_message(
                    job.chat_id,
                    "🛑 Codex work cancelled.",
                    job.reply_to_message_id,
                )?;
            } else {
                send_long_message(
                    tg,
                    job.chat_id,
                    &format!("⚠️ {} failed:\n{err}", job.provider_model.provider.label()),
                    job.reply_to_message_id,
                )?;
            }
            return Ok(());
        }
    };
    unregister_active_cancel(cancellations, &key, &cancel);
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_current_session(&key, state.generation, session_id)?;
    }
    logs::info(format_args!(
        "job success chat={} elapsed_ms={} final_chars={} session={}",
        job.chat_id,
        started.elapsed().as_millis(),
        output.final_text.chars().count(),
        session_label(output.session_id.as_deref().unwrap_or(""))
    ));
    match final_delivery(&output.final_text) {
        FinalDelivery::Reaction => {
            let _ = tg.delete_message(job.chat_id, stream_message_id);
            react_ok(tg, job.chat_id, job.reply_to_message_id)
        }
        FinalDelivery::Message(final_text) => {
            let final_text = redact_private_data(&final_text);
            let parts = split_telegram_message(&final_text);
            if let Some(first) = parts.first() {
                send_final_message(tg, &job, stream_message_id, first)?;
                for part in parts.iter().skip(1) {
                    tg.send_message(job.chat_id, part, 0)?;
                }
            }
            Ok(())
        }
    }
}

fn cancellation_epoch(cancellations: &CancellationState, key: &SessionKey) -> u64 {
    cancellations
        .lock()
        .unwrap()
        .get(key)
        .map(|entry| entry.epoch)
        .unwrap_or(0)
}

fn cancel_key(cancellations: &CancellationState, key: &SessionKey) -> usize {
    let mut cancellations = cancellations.lock().unwrap();
    let entry = cancellations
        .entry(key.clone())
        .or_insert(CancellationEntry {
            epoch: 0,
            active: Vec::new(),
        });
    entry.epoch = entry.epoch.saturating_add(1);
    entry.active.retain(|cancel| Arc::strong_count(cancel) > 1);
    for cancel in &entry.active {
        cancel.store(true, Ordering::SeqCst);
    }
    entry.active.len()
}

fn register_active_cancel(cancellations: &CancellationState, key: &SessionKey) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut cancellations = cancellations.lock().unwrap();
    let entry = cancellations
        .entry(key.clone())
        .or_insert(CancellationEntry {
            epoch: 0,
            active: Vec::new(),
        });
    entry.active.push(cancel.clone());
    cancel
}

fn unregister_active_cancel(
    cancellations: &CancellationState,
    key: &SessionKey,
    cancel: &Arc<AtomicBool>,
) {
    if let Some(entry) = cancellations.lock().unwrap().get_mut(key) {
        entry.active.retain(|active| !Arc::ptr_eq(active, cancel));
    }
}

fn is_job_cancelled(cancellations: &CancellationState, job: &Job) -> bool {
    let key = SessionKey::Chat {
        chat_id: job.chat_id,
        thread_id: job.thread_id,
    };
    cancellation_epoch(cancellations, &key) > job.cancel_epoch
}

const FINAL_MESSAGE_EFFECT_ID: &str = "5107584321108051014";

fn send_final_message(
    tg: &impl TelegramApi,
    job: &Job,
    stream_message_id: i64,
    first: &str,
) -> Result<(), String> {
    let _ = tg.delete_message(job.chat_id, stream_message_id);
    let first = redact_private_data(first);
    match tg.send_message_with_effect(
        job.chat_id,
        &first,
        job.reply_to_message_id,
        FINAL_MESSAGE_EFFECT_ID,
    ) {
        Ok(message) => {
            if message.effect_id.as_deref() != Some(FINAL_MESSAGE_EFFECT_ID) {
                logs::warn(format_args!(
                    "telegram final effect missing chat={} message={} requested_effect={} returned_effect={}",
                    job.chat_id,
                    message.message_id,
                    FINAL_MESSAGE_EFFECT_ID,
                    message.effect_id.as_deref().unwrap_or("<none>")
                ));
            }
            Ok(())
        }
        Err(err) => {
            logs::warn(format_args!(
                "telegram final effect failed chat={} effect={} error={}",
                job.chat_id, FINAL_MESSAGE_EFFECT_ID, err
            ));
            tg.send_message(job.chat_id, &first, job.reply_to_message_id)
        }
    }
}

fn single_line(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join(" | ")
}

fn start_typing_loop<T: TelegramApi>(tg: &T, chat_id: i64) -> TypingLoop {
    let tg = tg.clone();
    let (stop, stopped) = mpsc::channel();
    let handle = thread::spawn(move || loop {
        let _ = tg.send_chat_action(chat_id, "typing");
        if stopped.recv_timeout(typing_refresh_interval()).is_ok() {
            break;
        }
    });
    TypingLoop {
        stop: Some(stop),
        handle: Some(handle),
    }
}

fn empty_final_text(text: &str) -> String {
    if text.trim().is_empty() {
        "📭 Codex finished with no final text.".to_string()
    } else {
        text.to_string()
    }
}

fn final_delivery(text: &str) -> FinalDelivery {
    if is_ok_response(text) {
        FinalDelivery::Reaction
    } else {
        FinalDelivery::Message(empty_final_text(text))
    }
}

fn stream_preview(text: &str) -> String {
    let redacted = redact_private_data(text);
    let text = redacted.trim();
    if text.is_empty() {
        return "⏳ Codex is thinking…".to_string();
    }
    let max = 3800;
    if text.chars().count() <= max {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(max)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("…\n{tail}")
}

fn send_long_message(
    tg: &impl TelegramApi,
    chat_id: i64,
    text: &str,
    reply_to_message_id: i64,
) -> Result<(), String> {
    let text = redact_private_data(text);
    for (index, part) in split_telegram_message(&text).into_iter().enumerate() {
        let reply = if index == 0 { reply_to_message_id } else { 0 };
        tg.send_message(chat_id, &part, reply)?;
    }
    Ok(())
}

fn handle_update_command(cfg: &Config, tg: &impl TelegramApi, msg: &Message) -> Result<(), String> {
    let message = match start_gateway_update(cfg) {
        Ok(GatewayUpdateStart::Started) => {
            "⬆️ Updating gateway in the background. Running `git pull`, Homebrew maintenance, Foundry update, then `./setup`. Details go to `gateway/logs/update.log`.".to_string()
        }
        Ok(GatewayUpdateStart::AlreadyRunning) => {
            "⏳ Gateway update already running. Details go to `gateway/logs/update.log`."
                .to_string()
        }
        Err(err) => format!("⚠️ Gateway update failed to start: {err}"),
    };
    tg.send_message(msg.chat.id, &message, msg.message_id)?;
    Ok(())
}

fn restart_gateway(launchd_target: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", launchd_target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayUpdateStart {
    Started,
    AlreadyRunning,
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayUpdateLockStatus {
    Absent,
    Active,
    Stale,
}

fn start_gateway_update(cfg: &Config) -> Result<GatewayUpdateStart, String> {
    let lock_file = gateway_update_lock_file(cfg);
    if gateway_update_lock_status(&lock_file)? == GatewayUpdateLockStatus::Active {
        return Ok(GatewayUpdateStart::AlreadyRunning);
    }

    remove_gateway_update_lock(&lock_file)?;
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create update state dir: {err}"))?;
    fs::write(&lock_file, format!("pending {}\n", current_unix_seconds()))
        .map_err(|err| format!("write update lock: {err}"))?;

    if let Err(err) = submit_gateway_update(&lock_file) {
        let _ = fs::remove_file(&lock_file);
        return Err(err);
    }

    Ok(GatewayUpdateStart::Started)
}

fn gateway_update_lock_file(cfg: &Config) -> std::path::PathBuf {
    cfg.state_dir.join("update.lock")
}

fn gateway_update_lock_status(lock_file: &Path) -> Result<GatewayUpdateLockStatus, String> {
    let text = match fs::read_to_string(lock_file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GatewayUpdateLockStatus::Absent);
        }
        Err(err) => return Err(format!("read update lock: {err}")),
    };
    let mut parts = text.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let value = parts.next().unwrap_or_default();

    match kind {
        "pid" => Ok(value
            .parse::<u32>()
            .ok()
            .filter(|pid| process_is_running(*pid))
            .map(|_| GatewayUpdateLockStatus::Active)
            .unwrap_or(GatewayUpdateLockStatus::Stale)),
        "pending" => Ok(value
            .parse::<u64>()
            .ok()
            .filter(|seconds| {
                current_unix_seconds().saturating_sub(*seconds)
                    < GATEWAY_UPDATE_PENDING_LOCK_TTL_SECS
            })
            .map(|_| GatewayUpdateLockStatus::Active)
            .unwrap_or(GatewayUpdateLockStatus::Stale)),
        _ => Ok(GatewayUpdateLockStatus::Stale),
    }
}

fn remove_gateway_update_lock(lock_file: &Path) -> Result<(), String> {
    match fs::remove_file(lock_file) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove update lock: {err}")),
    }
}

fn process_is_running(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(not(test))]
fn submit_gateway_update(lock_file: &Path) -> Result<(), String> {
    let _ = Command::new("/bin/launchctl")
        .args(["remove", GATEWAY_UPDATE_JOB_LABEL])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let status = gateway_update_command(lock_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("run launchctl submit: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("launchctl submit exited with {status}"))
    }
}

#[cfg(test)]
fn submit_gateway_update(_lock_file: &Path) -> Result<(), String> {
    Ok(())
}

fn gateway_update_command(lock_file: &Path) -> Command {
    let mut command = Command::new("/bin/launchctl");
    command
        .args([
            "submit",
            "-l",
            GATEWAY_UPDATE_JOB_LABEL,
            "-o",
            "/dev/null",
            "-e",
            "/dev/null",
            "--",
            "/bin/zsh",
            "-lc",
            GATEWAY_UPDATE_SCRIPT,
            "gateway-update",
            GATEWAY_UPDATE_JOB_LABEL,
        ])
        .arg(lock_file)
        .arg(gateway_root());
    command
}

fn gateway_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

pub const fn typing_refresh_interval() -> Duration {
    TYPING_REFRESH_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::{Chat, Document, PhotoSize, User};
    use std::collections::VecDeque;
    use std::ffi::OsStr;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use tempfile::tempdir;

    #[test]
    fn offset_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");

        write_offset(&path, 42).unwrap();

        assert_eq!(read_offset(&path), 42);
    }

    #[test]
    fn invalid_offset_returns_zero() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");
        std::fs::write(&path, "bad").unwrap();

        assert_eq!(read_offset(&path), 0);
    }

    #[test]
    fn offset_advances_monotonically() {
        assert_eq!(advance_offset(10, 3), 10);
        assert_eq!(advance_offset(10, 12), 13);
    }

    #[test]
    fn initial_offset_skips_highest_update() {
        let updates = vec![
            crate::telegram::Update {
                update_id: 4,
                message: None,
                callback_query: None,
            },
            crate::telegram::Update {
                update_id: 9,
                message: None,
                callback_query: None,
            },
        ];

        assert_eq!(skip_offset(&updates), 10);
    }

    #[test]
    fn message_text_prefers_text_then_caption() {
        assert_eq!(message_text(" hello ", "caption").unwrap(), "hello");
        assert_eq!(message_text("", " caption ").unwrap(), "caption");
        assert_eq!(message_text("", "").unwrap_err(), "📝 Text messages only.");
    }

    #[test]
    fn typing_refreshes_before_telegram_expires() {
        assert!(typing_refresh_interval() < Duration::from_secs(5));
    }

    #[test]
    fn ok_final_text_uses_reaction_instead_of_message() {
        assert_eq!(final_delivery("OK"), FinalDelivery::Reaction);
        assert_eq!(final_delivery(" ok\n"), FinalDelivery::Reaction);
        assert_eq!(
            final_delivery("done"),
            FinalDelivery::Message("done".to_string())
        );
        assert_eq!(
            final_delivery(""),
            FinalDelivery::Message("📭 Codex finished with no final text.".to_string())
        );
    }

    #[test]
    fn run_with_client_initializes_state_sends_status_and_persists_initial_offset() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.push_update(Ok(vec![Update {
            update_id: 4,
            message: None,
            callback_query: None,
        }]));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg.clone(), tg.clone()).unwrap_err();

        assert_eq!(err, "stop");
        assert!(cfg.state_dir.exists());
        assert!(cfg.chat_state_dir.exists());
        let mut state_entries = fs::read_dir(&cfg.state_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        state_entries.sort();
        assert_eq!(
            state_entries,
            vec!["chats".to_string(), "telegram.offset".to_string()]
        );
        assert_eq!(read_offset(&cfg.offset_file), 5);
        assert!(tg.calls().contains(&Call::Sync(vec![42])));
        assert!(tg.calls().iter().any(|call| {
            matches!(call, Call::Send { chat_id: 42, reply_to: 0, text } if text.contains("🤖 Model: gpt-test"))
        }));
        assert!(tg.calls().contains(&Call::GetUpdates {
            offset: 0,
            timeout: 0
        }));
        assert!(tg.calls().contains(&Call::GetUpdates {
            offset: 5,
            timeout: 50
        }));
    }

    #[test]
    fn public_run_reports_state_dir_creation_errors() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(cfg.state_dir.parent().unwrap()).unwrap();
        fs::write(&cfg.state_dir, "not a dir").unwrap();

        let err = run(cfg).unwrap_err();

        assert!(err.contains("create state dir"));
    }

    #[test]
    fn run_with_client_handles_polled_command_messages() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Ok(vec![Update {
            update_id: 10,
            message: Some(message(42, 11, "/help")),
            callback_query: None,
        }]));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg.clone(), tg.clone()).unwrap_err();

        assert_eq!(err, "stop");
        assert_eq!(read_offset(&cfg.offset_file), 11);
        assert!(tg.sent_text().iter().any(|text| {
            text.contains("❓ Unknown directive")
                && text.contains("/status")
                && !text.contains("/help")
        }));
    }

    #[test]
    fn run_with_client_continues_when_message_delivery_fails() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Ok(vec![Update {
            update_id: 10,
            message: Some(message(42, 11, "/help")),
            callback_query: None,
        }]));
        tg.push_update(Err("stop".to_string()));
        tg.fail_sends("send failed");

        let err = run_with_client(cfg.clone(), tg).unwrap_err();

        assert_eq!(err, "stop");
        assert_eq!(read_offset(&cfg.offset_file), 11);
    }

    #[test]
    fn run_with_client_continues_when_command_sync_fails() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.fail_sync("too many requests");
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg, tg.clone()).unwrap_err();

        assert_eq!(err, "stop");
        assert!(tg.calls().contains(&Call::Sync(vec![42])));
        assert!(tg.calls().contains(&Call::GetUpdates {
            offset: 0,
            timeout: 0
        }));
    }

    #[test]
    fn run_with_client_continues_when_startup_status_send_fails() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.fail_sends("send failed");
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg, tg).unwrap_err();

        assert_eq!(err, "stop");
    }

    #[test]
    fn run_with_client_retries_telegram_get_updates_conflicts() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Err(
            "Conflict: terminated by other getUpdates request; make sure that only one bot instance is running"
                .to_string(),
        ));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg, tg.clone()).unwrap_err();

        assert_eq!(err, "stop");
        assert_eq!(
            tg.calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    Call::GetUpdates {
                        offset: 0,
                        timeout: 50
                    }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn run_with_client_retries_transient_telegram_get_updates_request_failures() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let tg = FakeTelegram::new();
        tg.push_update(Ok(Vec::new()));
        tg.push_update(Err(
            "telegram getUpdates request failed: Network Error: timed out reading response"
                .to_string(),
        ));
        tg.push_update(Err("stop".to_string()));

        let err = run_with_client(cfg, tg.clone()).unwrap_err();

        assert_eq!(err, "stop");
        assert_eq!(
            tg.calls()
                .iter()
                .filter(|call| matches!(
                    call,
                    Call::GetUpdates {
                        offset: 0,
                        timeout: 50
                    }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn handle_message_authorizes_validates_queues_and_reports_full_queue() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);

        handle_message(
            &cfg,
            &tg,
            &store,
            &selections,
            &tx,
            message(9, 1, "ignored"),
        )
        .unwrap();
        assert!(tg.calls().is_empty());

        handle_message(&cfg, &tg, &store, &selections, &tx, message(42, 2, "  ")).unwrap();
        assert!(tg.calls().iter().any(|call| {
            matches!(call, Call::Send { text, reply_to: 2, .. } if text == "📝 Text messages only.")
        }));

        handle_message(
            &cfg,
            &tg,
            &store,
            &selections,
            &tx,
            message(42, 3, "run this"),
        )
        .unwrap();
        let job = rx.recv().unwrap();
        assert_eq!(job.bot_token, "token");
        assert_eq!(job.chat_id, 42);
        assert_eq!(job.reply_to_message_id, 3);
        assert_eq!(job.prompt, "run this");
        assert!(tg.calls().contains(&Call::Action {
            chat_id: 42,
            action: "typing".to_string()
        }));

        let (full_tx, _full_rx) = mpsc::sync_channel(0);
        handle_message(
            &cfg,
            &tg,
            &store,
            &selections,
            &full_tx,
            message(42, 4, "queued"),
        )
        .unwrap();
        assert!(tg.calls().iter().any(|call| {
            matches!(call, Call::Send { text, reply_to: 4, .. } if text.contains("🚦 Codex queue is full"))
        }));
    }

    #[test]
    fn handle_message_adds_replied_message_text_to_prompt() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut msg = message(42, 6, "what do you think?");
        msg.reply_to_message = Some(Box::new(message(42, 5, "original claim")));

        handle_message(&cfg, &tg, &store, &selections, &tx, msg).unwrap();

        let job = rx.recv().unwrap();
        assert_eq!(
            job.prompt,
            "Telegram reply context:\noriginal claim\n\nUser message:\nwhat do you think?"
        );
    }

    #[test]
    fn handle_message_downloads_photo_and_document_for_codex() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.push_download(Ok(b"photo bytes".to_vec()));
        tg.push_download(Ok(b"document bytes".to_vec()));
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut msg = message(42, 6, "summarize these");
        msg.photo = vec![
            PhotoSize {
                file_id: "photo-small".to_string(),
                width: 32,
                height: 32,
                file_size: Some(100),
            },
            PhotoSize {
                file_id: "photo-large".to_string(),
                width: 640,
                height: 480,
                file_size: Some(1000),
            },
        ];
        msg.document = Some(Document {
            file_id: "doc-1".to_string(),
            file_name: "notes.txt".to_string(),
            mime_type: "text/plain".to_string(),
        });

        handle_message(&cfg, &tg, &store, &selections, &tx, msg).unwrap();

        let job = rx.recv().unwrap();
        assert_eq!(job.image_paths.len(), 1);
        assert_eq!(job.file_paths.len(), 1);
        let image_path = &job.image_paths[0];
        let file_path = &job.file_paths[0];
        let attachment_dir = image_path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert!(attachment_dir.starts_with("attach-"));
        assert!(!attachment_dir.starts_with("telegram-attachments-"));
        assert_eq!(fs::read(image_path).unwrap(), b"photo bytes");
        assert_eq!(fs::read(file_path).unwrap(), b"document bytes");
        assert!(job.prompt.starts_with("summarize these"));
        assert!(job.prompt.contains("Telegram file attachments:"));
        assert!(job.prompt.contains(file_path.to_str().unwrap()));
        assert!(tg.calls().contains(&Call::Download {
            file_id: "photo-large".to_string(),
            path: image_path.clone(),
        }));
        assert!(tg.calls().contains(&Call::Download {
            file_id: "doc-1".to_string(),
            path: file_path.clone(),
        }));
    }

    #[test]
    fn handle_message_ignores_allowed_non_private_chat() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut msg = message(42, 5, "run this");
        msg.chat.kind = "group".to_string();

        handle_message(&cfg, &tg, &store, &selections, &tx, msg).unwrap();

        assert!(tg.calls().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_message_ignores_private_chat_when_sender_does_not_match_chat() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut msg = message(42, 5, "run this");
        msg.from = Some(User {
            id: 7,
            username: String::new(),
        });

        handle_message(&cfg, &tg, &store, &selections, &tx, msg).unwrap();

        assert!(tg.calls().is_empty());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_message_propagates_queue_full_send_errors() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.fail_sends("send failed");
        let selections = RuntimeSelections::default();
        let (full_tx, _full_rx) = mpsc::sync_channel(0);

        let err = handle_message(
            &cfg,
            &tg,
            &store,
            &selections,
            &full_tx,
            message(42, 4, "queued"),
        )
        .unwrap_err();

        assert_eq!(err, "send failed");
    }

    #[test]
    fn handle_command_covers_directive_responses() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-invalid-title"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'OK\n' > "$out"
printf 'session id: session-12345678\n' >&2
"#,
            ),
        );
        let msg = message(42, 10, "/help");
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };

        handle_command(&cfg, &tg, &store, &selections, &msg, "/log", "/log").unwrap();
        fs::create_dir_all(cfg.gateway_log_file.parent().unwrap()).unwrap();
        fs::write(&cfg.gateway_log_file, "one\ntwo\nthree\n").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/log 2", "/log").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/new", "/new").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/model", "/model").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/model 0", "/model").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/resume", "/resume").unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/resume missing",
            "/resume",
        )
        .unwrap();
        store
            .save_current_session(&key, store.load(&key).generation, "session-12345678")
            .unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/resume session-12345678",
            "/resume",
        )
        .unwrap();
        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename",
            "/rename",
        )
        .unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename work",
            "/rename",
        )
        .unwrap();
        assert_session_name_eventually(&store, &key, "session-12345678", "work");
        handle_command(&cfg, &tg, &store, &selections, &msg, "/list", "/list").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/stop", "/stop").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/help", "/help").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/config", "/config").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/update", "/update").unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/commands",
            "/commands",
        )
        .unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/status", "/status").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/restart", "/restart").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/wat", "/wat").unwrap();

        let sent = tg.sent_text();
        assert!(!sent.iter().any(|text| text.contains("bot_token=token")));
        assert!(sent
            .iter()
            .any(|text| text == "📭 No gateway log available."));
        assert!(sent.iter().any(|text| text.contains("two\n\nthree")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🆕 New session ready")));
        assert!(sent.iter().any(|text| text.contains("🤖 Model: gpt-test")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🤖 Selected Codex: gpt-test")));
        assert!(sent
            .iter()
            .any(|text| text.contains("📭 No saved sessions yet")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🔎 No saved session matches")));
        assert!(sent.iter().any(|text| text.contains("↩️ Resumed session")));
        assert!(!sent
            .iter()
            .any(|text| text.contains("🏷️ Naming current session")));
        assert!(!sent
            .iter()
            .any(|text| text.contains("invalid session name")));
        assert!(!sent.iter().any(|text| text.contains("🏷️ Renamed session")));
        assert!(tg.calls().contains(&Call::Reaction {
            chat_id: 42,
            message_id: 10,
            emoji: "👍".to_string(),
        }));
        assert!(sent.iter().any(|text| text.contains("💾 Saved sessions:")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🛑") && text.contains("queued work")));
        assert!(sent.iter().any(|text| text.contains("/status")));
        assert!(sent.iter().any(|text| text
            == "⬆️ Updating gateway in the background. Running `git pull`, Homebrew maintenance, Foundry update, then `./setup`. Details go to `gateway/logs/update.log`."));
        assert!(!sent.iter().any(|text| text.contains("/commands")));
        assert!(!sent
            .iter()
            .any(|text| text.starts_with("🧭 Supported directives:")));
        assert_eq!(
            sent.iter()
                .filter(|text| text.contains("❓ Unknown directive"))
                .count(),
            4
        );
        assert!(sent.iter().any(|text| text.contains("🧠 Codex:")));
        assert!(sent.iter().any(|text| text == "🔄 Restarting gateway."));
        assert!(sent
            .iter()
            .any(|text| text.contains("❓ Unknown directive")));
    }

    #[test]
    fn update_command_reports_existing_active_update_lock() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::write(
            gateway_update_lock_file(&cfg),
            format!("pid {}\n", std::process::id()),
        )
        .unwrap();
        let tg = FakeTelegram::new();
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/update");

        handle_command(&cfg, &tg, &store, &selections, &msg, "/update", "/update").unwrap();

        assert!(tg.sent_text().iter().any(|text| {
            text == "⏳ Gateway update already running. Details go to `gateway/logs/update.log`."
        }));
    }

    #[test]
    fn gateway_update_command_submits_stable_launchd_job_with_lock_cleanup() {
        let lock_file = Path::new("/tmp/gateway-state/update.lock");
        let command = gateway_update_command(lock_file);
        let args = command.get_args().collect::<Vec<_>>();

        assert_eq!(command.get_program(), OsStr::new("/bin/launchctl"));
        assert_eq!(
            &args[..8],
            vec![
                OsStr::new("submit"),
                OsStr::new("-l"),
                OsStr::new("ai.gateway.update"),
                OsStr::new("-o"),
                OsStr::new("/dev/null"),
                OsStr::new("-e"),
                OsStr::new("/dev/null"),
                OsStr::new("--"),
            ]
        );
        assert_eq!(args[8], OsStr::new("/bin/zsh"));
        assert_eq!(args[9], OsStr::new("-lc"));
        let script = args[10].to_string_lossy();
        assert!(script.contains("gateway_update_label=\"$1\""));
        assert!(script.contains("gateway_update_lock=\"$2\""));
        assert!(script.contains("gateway_update_root=\"$3\""));
        assert!(script.contains("print -r -- \"$$\" > \"$gateway_update_lock\""));
        assert!(script.contains("set -o pipefail"));
        assert!(script.contains("brew update"));
        assert!(script.contains("brew upgrade"));
        assert!(script.contains("brew cleanup"));
        assert!(script.contains("https://raw.githubusercontent.com/foundry-rs/foundry/refs/heads/master/foundryup/foundryup"));
        assert!(script.contains("git pull"));
        assert!(script.contains("./setup"));
        assert!(script.contains("gateway_update_code=$?"));
        assert!(script.contains("gateway_update_log=\"${gateway_update_lock:h}/logs/update.log\""));
        let git_pull = script.find("git pull").unwrap();
        let brew_update = script.find("brew update").unwrap();
        let brew_upgrade = script.find("brew upgrade").unwrap();
        let brew_cleanup = script.find("brew cleanup").unwrap();
        let foundry_update = script
            .find("https://raw.githubusercontent.com/foundry-rs/foundry/refs/heads/master/foundryup/foundryup")
            .unwrap();
        let setup = script.find("./setup").unwrap();
        assert!(git_pull < brew_update);
        assert!(brew_update < brew_upgrade);
        assert!(brew_upgrade < brew_cleanup);
        assert!(brew_cleanup < foundry_update);
        assert!(foundry_update < setup);
        let lock_cleanup = script.find("rm -f \"$gateway_update_lock\"").unwrap();
        let label_cleanup = script
            .find("/bin/launchctl remove \"$gateway_update_label\"")
            .unwrap();
        assert!(lock_cleanup < label_cleanup);
        assert_eq!(args[11], OsStr::new("gateway-update"));
        assert_eq!(args[12], OsStr::new("ai.gateway.update"));
        assert_eq!(args[13], OsStr::new("/tmp/gateway-state/update.lock"));
        assert_eq!(args[14], OsStr::new(env!("CARGO_MANIFEST_DIR")));
    }

    #[test]
    fn resume_numeric_argument_selects_list_index() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/resume 2");
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        seed_session_history(&store, &key);

        handle_command(&cfg, &tg, &store, &selections, &msg, "/resume 2", "/resume").unwrap();

        let state = store.load(&key);
        assert_eq!(state.session_id.as_deref(), Some("bbbbbbbb-previous"));
        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text.contains("↩️ Resumed session bbbbbbbb")));
    }

    #[test]
    fn resume_zero_and_empty_arguments_show_session_list() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/resume");
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        seed_session_history(&store, &key);

        handle_command(&cfg, &tg, &store, &selections, &msg, "/resume", "/resume").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/resume 0", "/resume").unwrap();

        let sent = tg.sent_text();
        assert_eq!(
            sent.iter()
                .filter(|text| text.contains("💾 Saved sessions:"))
                .count(),
            2
        );
        assert!(!sent.iter().any(|text| text.contains("Usage: /resume")));
    }

    #[test]
    fn rename_without_name_starts_auto_rename_without_waiting_or_sending_telegram() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-title"),
                r#"#!/bin/sh
sleep 0.3
printf '%s\n' "$@" > codex-title.args
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat > codex-title.prompt
printf '  session-name  \n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/rename");

        let started = Instant::now();
        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename",
            "/rename",
        )
        .unwrap();

        assert!(started.elapsed() < Duration::from_millis(250));
        assert_no_auto_rename_telegram(&tg);
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "session-name");
        let args = fs::read_to_string(dir.path().join("codex-title.args")).unwrap();
        assert!(args
            .lines()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["-m", "gpt-5.4-mini"]));
        let prompt = fs::read_to_string(dir.path().join("codex-title.prompt")).unwrap();
        assert!(prompt.contains("lowercase single-word"));
        assert!(prompt.contains("session-name"));
    }

    #[test]
    fn rename_without_name_auto_renames_even_when_current_session_already_named() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-retitle"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'refreshed-name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        store.rename_current(&key, "old-name").unwrap();
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/rename");

        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename",
            "/rename",
        )
        .unwrap();

        assert_no_auto_rename_telegram(&tg);
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "refreshed-name");
    }

    #[test]
    fn status_starts_auto_rename_without_waiting_or_sending_rename_messages() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-status-title"),
                r#"#!/bin/sh
sleep 0.3
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'status-name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/status");

        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/status",
            "/status",
        )
        .unwrap();

        let sent = tg.sent_text();
        assert_no_auto_rename_telegram(&tg);
        assert!(sent
            .iter()
            .any(|text| text.contains("🧵 Session: aaaaaaaa")));
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "status-name");
    }

    #[test]
    fn startup_starts_auto_rename_without_waiting_or_sending_telegram() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-startup-title"),
                r#"#!/bin/sh
sleep 0.3
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'startup-name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();

        let started = Instant::now();
        assert!(auto_rename_startup_session(&cfg, &codex, &tg, &store, 42, &key).unwrap());

        assert!(started.elapsed() < Duration::from_millis(250));
        assert_no_auto_rename_telegram(&tg);
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "startup-name");
    }

    #[test]
    fn rename_without_name_rejects_invalid_codex_session_name() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-invalid-title"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'Generated Session Name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
printf done > codex-invalid-title.done
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/rename");

        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename",
            "/rename",
        )
        .unwrap();

        assert_file_eventually(&dir.path().join("codex-invalid-title.done"));
        let state = store.load(&key);
        assert_eq!(state.sessions[0].name, None);
        assert_no_auto_rename_telegram(&tg);
    }

    #[test]
    fn rename_with_name_reacts_without_sending_telegram_and_overwrites_existing_name() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        store.rename_current(&key, "old-name").unwrap();
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/rename new-name");

        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename new-name",
            "/rename",
        )
        .unwrap();

        assert!(tg.sent_text().is_empty());
        assert!(tg.calls().contains(&Call::Reaction {
            chat_id: 42,
            message_id: 10,
            emoji: "👍".to_string(),
        }));
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "new-name");
    }

    #[test]
    fn explicit_rename_wins_over_pending_auto_rename() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-manual-title"),
                r#"#!/bin/sh
sleep 0.3
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'auto-name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
printf done > codex-manual-title.done
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/rename");

        handle_command_with_codex(
            &cfg,
            &codex,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename",
            "/rename",
        )
        .unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &message(42, 11, "/rename manual"),
            "/rename manual",
            "/rename",
        )
        .unwrap();

        assert_file_eventually(&dir.path().join("codex-manual-title.done"));
        assert_session_name_remains_for(
            &store,
            &key,
            "aaaaaaaa-current",
            "manual",
            Duration::from_millis(300),
        );
        let sent = tg.sent_text();
        assert!(!sent
            .iter()
            .any(|text| text.contains("🏷️ Naming current session")));
        assert!(!sent.iter().any(|text| text.contains("🏷️ Renamed session")));
        assert!(tg.calls().contains(&Call::Reaction {
            chat_id: 42,
            message_id: 11,
            emoji: "👍".to_string(),
        }));
    }

    #[test]
    fn auto_session_name_validates_lowercase_hyphenated_names() {
        assert_eq!(auto_session_name("session"), Some("session".to_string()));
        assert_eq!(
            auto_session_name("session2-name3"),
            Some("session2-name3".to_string())
        );
        assert_eq!(
            auto_session_name("`session-name`"),
            Some("session-name".to_string())
        );
        assert_eq!(auto_session_name("Session Name"), None);
        assert_eq!(auto_session_name("session_name"), None);
        assert_eq!(auto_session_name("session--name"), None);
        assert_eq!(auto_session_name("-session"), None);
        assert_eq!(auto_session_name("session-"), None);
    }

    #[test]
    fn new_starts_auto_rename_without_waiting_before_reset() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-title-new"),
                r#"#!/bin/sh
sleep 0.3
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'session-name\n' > "$out"
printf 'session id: aaaaaaaa-current\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        assert!(store
            .save_current_session(&key, 0, "aaaaaaaa-current")
            .unwrap());
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/new");

        let started = Instant::now();
        handle_command_with_codex(&cfg, &codex, &tg, &store, &selections, &msg, "/new", "/new")
            .unwrap();

        assert!(started.elapsed() < Duration::from_millis(250));
        let state = store.load(&key);
        assert_eq!(state.session_id, None);
        let sent = tg.sent_text();
        assert_no_auto_rename_telegram(&tg);
        assert!(sent
            .iter()
            .any(|text| text.contains("🆕 New session ready")));
        assert_session_name_eventually(&store, &key, "aaaaaaaa-current", "session-name");
    }

    #[test]
    fn model_command_shows_configured_model_buttons() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/model");

        handle_command(&cfg, &tg, &store, &selections, &msg, "/model", "/model").unwrap();

        assert!(tg.calls().iter().any(|call| {
            matches!(
                call,
                Call::SendKeyboard { text, buttons, .. }
                    if text == "🤖 Select model:"
                        && buttons.iter().map(|button| button.text.as_str()).collect::<Vec<_>>()
                            == vec![
                                "Codex: gpt-test",
                                "Claude: claude-test",
                                "OpenRouter: openrouter/test"
                            ]
                        && buttons.iter().map(|button| button.callback_data.as_str()).collect::<Vec<_>>()
                            == vec!["model:0", "model:1", "model:2"]
            )
        }));
    }

    #[test]
    fn model_command_reports_usage_for_invalid_index() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/model nope");

        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/model nope",
            "/model",
        )
        .unwrap();

        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text == "🧭 Usage: /model or /model 0..2"));
    }

    #[test]
    fn model_index_selection_is_in_memory_and_resets_on_new_session() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);
        let msg = message(42, 10, "/model");

        handle_command(&cfg, &tg, &store, &selections, &msg, "/model 1", "/model").unwrap();
        handle_message(&cfg, &tg, &store, &selections, &tx, message(42, 11, "run")).unwrap();
        let selected_job = rx.recv().unwrap();
        assert_eq!(selected_job.provider_model.provider, Provider::Claude);
        assert_eq!(selected_job.provider_model.model, "claude-test");
        assert!(!cfg.gateway_config_file.exists());

        handle_command(&cfg, &tg, &store, &selections, &msg, "/new", "/new").unwrap();
        handle_message(
            &cfg,
            &tg,
            &store,
            &selections,
            &tx,
            message(42, 12, "run again"),
        )
        .unwrap();
        let default_job = rx.recv().unwrap();
        assert_eq!(default_job.provider_model.provider, Provider::Codex);
        assert_eq!(default_job.provider_model.model, "gpt-test");
    }

    #[test]
    fn callback_query_selects_model_slot() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);

        handle_callback_query(
            &cfg,
            &tg,
            &store,
            &selections,
            callback_query("callback-1", message(42, 10, "/model"), "model:2"),
        )
        .unwrap();
        handle_message(&cfg, &tg, &store, &selections, &tx, message(42, 11, "run")).unwrap();

        assert!(tg.calls().contains(&Call::AnswerCallback {
            callback_query_id: "callback-1".to_string(),
            text: "Selected OpenRouter: openrouter/test".to_string(),
        }));
        let job = rx.recv().unwrap();
        assert_eq!(job.provider_model.provider, Provider::Openrouter);
        assert_eq!(job.provider_model.model, "openrouter/test");
    }

    #[test]
    fn callback_query_ignores_private_chat_when_sender_does_not_match_chat() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let (tx, rx) = mpsc::sync_channel(1);

        let mut query = callback_query("callback-1", message(42, 10, "/model"), "model:2");
        query.from.id = 7;
        handle_callback_query(&cfg, &tg, &store, &selections, query).unwrap();
        assert!(tg.calls().is_empty());

        handle_message(&cfg, &tg, &store, &selections, &tx, message(42, 11, "run")).unwrap();

        let job = rx.recv().unwrap();
        assert_eq!(job.provider_model.provider, Provider::Codex);
        assert_eq!(job.provider_model.model, "gpt-test");
    }

    #[test]
    fn handle_command_reports_session_and_config_write_errors() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        let blocked = dir.path().join("blocked");
        fs::write(&blocked, "file").unwrap();
        cfg.chat_state_dir = blocked.join("chats");
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/new");

        handle_command(&cfg, &tg, &store, &selections, &msg, "/new", "/new").unwrap();
        handle_command(
            &cfg,
            &tg,
            &store,
            &selections,
            &msg,
            "/rename work",
            "/rename",
        )
        .unwrap();

        let sent = tg.sent_text();
        assert!(sent
            .iter()
            .any(|text| text.contains("⚠️ Failed to reset session")));
        assert!(!sent.iter().any(|text| text.contains("No current session")));
        assert!(tg.calls().contains(&Call::Reaction {
            chat_id: 42,
            message_id: 10,
            emoji: "👍".to_string(),
        }));
    }

    #[test]
    fn log_command_redacts_sensitive_values_before_sending() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 12, "/log 10");
        fs::create_dir_all(cfg.gateway_log_file.parent().unwrap()).unwrap();
        fs::write(
            &cfg.gateway_log_file,
            "OPENAI_API_KEY=sk-test-secret-value\nAuthorization: Bearer bearer-secret-token\n",
        )
        .unwrap();

        handle_command(&cfg, &tg, &store, &selections, &msg, "/log 10", "/log").unwrap();

        let sent = tg.sent_text().join("\n");
        assert!(!sent.contains("sk-test-secret-value"));
        assert!(!sent.contains("bearer-secret-token"));
        assert!(sent.contains("OPENAI_API_KEY=<redacted>"));
        assert!(sent.contains("Authorization: Bearer <redacted>"));
    }

    #[test]
    fn run_job_saves_sessions_and_uses_reaction_for_ok_results() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-ok"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'OK\n' > "$out"
printf 'session id: session-ok\n' >&2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job_with_codex(&cfg, &codex, &tg, &store, job("make it so")).unwrap();

        let calls = tg.calls();
        assert!(calls.contains(&Call::SendReturning {
            chat_id: 42,
            reply_to: 7,
            text: "🫧 Thinking…".to_string()
        }));
        assert!(calls.contains(&Call::Delete {
            chat_id: 42,
            message_id: 100
        }));
        assert!(calls.contains(&Call::Reaction {
            chat_id: 42,
            message_id: 7,
            emoji: "👍".to_string()
        }));
        assert_eq!(
            store
                .load(&SessionKey::Chat {
                    chat_id: 42,
                    thread_id: None,
                })
                .session_id
                .as_deref(),
            Some("session-ok")
        );
    }

    #[test]
    fn run_job_sends_final_message_and_falls_back_without_effect() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-final"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'final text\n' > "$out"
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.push_effect(Ok(message_with_effect(200, None)));

        run_job_with_codex(&cfg, &codex, &tg, &store, job("answer")).unwrap();

        assert!(tg.calls().contains(&Call::Effect {
            chat_id: 42,
            reply_to: 7,
            effect_id: FINAL_MESSAGE_EFFECT_ID.to_string(),
            text: "final text".to_string()
        }));

        let tg = FakeTelegram::new();
        tg.push_effect(Err("effect failed".to_string()));
        run_job_with_codex(&cfg, &codex, &tg, &store, job("answer")).unwrap();

        assert!(tg.sent_text().contains(&"final text".to_string()));
    }

    #[test]
    fn run_job_redacts_final_text_before_sending_to_telegram() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-secret"),
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'OPENAI_API_KEY=sk-test-secret-value\n' > "$out"
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job_with_codex(&cfg, &codex, &tg, &store, job("answer")).unwrap();

        let delivered = tg
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Effect { text, .. } => Some(text),
                _ => None,
            })
            .unwrap();
        assert!(!delivered.contains("sk-test-secret-value"));
        assert!(delivered.contains("OPENAI_API_KEY=<redacted>"));
    }

    #[test]
    fn run_job_edits_stream_preview_and_sends_split_final_parts() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let final_text = "a".repeat(crate::text::TELEGRAM_MESSAGE_LIMIT + 20);
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-streams"),
                &format!(
                    r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'first\n'
sleep 2
printf 'second\n'
printf '%s\n' '{}' > "$out"
"#,
                    final_text
                ),
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job_with_codex(&cfg, &codex, &tg, &store, job("stream")).unwrap();

        assert!(tg
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Edit { .. })));
        assert!(tg
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Send { reply_to: 0, .. })));
    }

    #[test]
    fn run_job_reports_codex_failures() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-fails"),
                r#"#!/bin/sh
printf 'boom\n' >&2
exit 2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job_with_codex(&cfg, &codex, &tg, &store, job("fail")).unwrap();

        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text.contains("⚠️ Codex failed:\nboom")));
    }

    #[test]
    fn run_job_propagates_codex_failure_delivery_errors() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-fails"),
                r#"#!/bin/sh
printf 'boom\n' >&2
exit 2
"#,
            ),
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.fail_sends("send failed");

        let err = run_job_with_codex(&cfg, &codex, &tg, &store, job("fail")).unwrap_err();

        assert_eq!(err, "send failed");
    }

    #[test]
    fn stream_preview_handles_empty_short_and_long_text() {
        assert_eq!(stream_preview(" \n"), "⏳ Codex is thinking…");
        assert_eq!(stream_preview(" hello "), "hello");

        let long = format!("{}tail", "a".repeat(3900));
        let preview = stream_preview(&long);

        assert!(preview.starts_with("…\n"));
        assert!(preview.ends_with("tail"));
        assert!(preview.chars().count() <= 3802);
    }

    #[test]
    fn telegram_client_trait_impl_delegates_to_inherent_methods() {
        let mut responses = vec![json_response(r#"{"ok":true,"result":[]}"#)];
        responses.extend((0..18).map(|_| json_response(r#"{"ok":true,"result":true}"#)));
        responses.extend([
            json_response(r#"{"ok":true,"result":{}}"#),
            json_response(telegram_message_json(101, None).as_str()),
            json_response(telegram_message_json(102, Some(FINAL_MESSAGE_EFFECT_ID)).as_str()),
            json_response(r#"{"ok":true,"result":true}"#),
            json_response(r#"{"ok":true,"result":true}"#),
            json_response(r#"{"ok":true,"result":{}}"#),
            json_response(r#"{"ok":true,"result":true}"#),
        ]);
        let server = MiniServer::new(responses);
        let client = TelegramClient::with_base_url(server.base_url);

        assert!(TelegramApi::get_updates(&client, 0, 0).unwrap().is_empty());
        TelegramApi::sync_my_commands(&client, &[42]).unwrap();
        TelegramApi::send_message(&client, 42, "hello", 7).unwrap();
        assert_eq!(
            TelegramApi::send_message_returning(&client, 42, "hello", 7).unwrap(),
            101
        );
        assert_eq!(
            TelegramApi::send_message_with_effect(&client, 42, "done", 7, FINAL_MESSAGE_EFFECT_ID)
                .unwrap()
                .message_id,
            102
        );
        TelegramApi::delete_message(&client, 42, 100).unwrap();
        TelegramApi::set_message_reaction(&client, 42, 7, "👍").unwrap();
        TelegramApi::edit_message_text(&client, 42, 100, "edit").unwrap();
        TelegramApi::send_chat_action(&client, 42, "typing").unwrap();
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Sync(Vec<i64>),
        GetUpdates {
            offset: i64,
            timeout: u64,
        },
        Send {
            chat_id: i64,
            reply_to: i64,
            text: String,
        },
        SendKeyboard {
            chat_id: i64,
            reply_to: i64,
            text: String,
            buttons: Vec<InlineKeyboardButton>,
        },
        AnswerCallback {
            callback_query_id: String,
            text: String,
        },
        SendReturning {
            chat_id: i64,
            reply_to: i64,
            text: String,
        },
        Effect {
            chat_id: i64,
            reply_to: i64,
            effect_id: String,
            text: String,
        },
        Delete {
            chat_id: i64,
            message_id: i64,
        },
        Download {
            file_id: String,
            path: PathBuf,
        },
        Reaction {
            chat_id: i64,
            message_id: i64,
            emoji: String,
        },
        Edit {
            chat_id: i64,
            message_id: i64,
            text: String,
        },
        Action {
            chat_id: i64,
            action: String,
        },
    }

    #[derive(Default)]
    struct FakeState {
        calls: Vec<Call>,
        updates: VecDeque<Result<Vec<Update>, String>>,
        effects: VecDeque<Result<Message, String>>,
        downloads: VecDeque<Result<Vec<u8>, String>>,
        sync_error: Option<String>,
        send_error: Option<String>,
        next_message_id: i64,
    }

    #[derive(Clone, Default)]
    struct FakeTelegram {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeTelegram {
        fn new() -> Self {
            let state = FakeState {
                next_message_id: 100,
                ..FakeState::default()
            };
            Self {
                state: Arc::new(Mutex::new(state)),
            }
        }

        fn push_update(&self, update: Result<Vec<Update>, String>) {
            self.state.lock().unwrap().updates.push_back(update);
        }

        fn push_effect(&self, effect: Result<Message, String>) {
            self.state.lock().unwrap().effects.push_back(effect);
        }

        fn push_download(&self, download: Result<Vec<u8>, String>) {
            self.state.lock().unwrap().downloads.push_back(download);
        }

        fn fail_sends(&self, err: &str) {
            self.state.lock().unwrap().send_error = Some(err.to_string());
        }

        fn fail_sync(&self, err: &str) {
            self.state.lock().unwrap().sync_error = Some(err.to_string());
        }

        fn calls(&self) -> Vec<Call> {
            self.state.lock().unwrap().calls.clone()
        }

        fn sent_text(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|call| match call {
                    Call::Send { text, .. } => Some(text),
                    _ => None,
                })
                .collect()
        }
    }

    impl TelegramApi for FakeTelegram {
        fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String> {
            let mut state = self.state.lock().unwrap();
            state.calls.push(Call::GetUpdates {
                offset,
                timeout: timeout_sec,
            });
            state
                .updates
                .pop_front()
                .unwrap_or_else(|| Err("stop".to_string()))
        }

        fn sync_my_commands(&self, chat_ids: &[i64]) -> Result<(), String> {
            let sync_error = {
                let mut state = self.state.lock().unwrap();
                state.calls.push(Call::Sync(chat_ids.to_vec()));
                state.sync_error.clone()
            };
            if let Some(err) = sync_error {
                return Err(err);
            }
            Ok(())
        }

        fn send_message(
            &self,
            chat_id: i64,
            text: &str,
            reply_to_message_id: i64,
        ) -> Result<(), String> {
            let send_error = {
                let mut state = self.state.lock().unwrap();
                state.calls.push(Call::Send {
                    chat_id,
                    reply_to: reply_to_message_id,
                    text: text.to_string(),
                });
                state.send_error.clone()
            };
            if let Some(err) = send_error {
                return Err(err);
            }
            Ok(())
        }

        fn send_message_with_inline_keyboard(
            &self,
            chat_id: i64,
            text: &str,
            reply_to_message_id: i64,
            buttons: &[InlineKeyboardButton],
        ) -> Result<(), String> {
            let send_error = {
                let mut state = self.state.lock().unwrap();
                state.calls.push(Call::SendKeyboard {
                    chat_id,
                    reply_to: reply_to_message_id,
                    text: text.to_string(),
                    buttons: buttons.to_vec(),
                });
                state.send_error.clone()
            };
            if let Some(err) = send_error {
                return Err(err);
            }
            Ok(())
        }

        fn answer_callback_query(&self, callback_query_id: &str, text: &str) -> Result<(), String> {
            self.state.lock().unwrap().calls.push(Call::AnswerCallback {
                callback_query_id: callback_query_id.to_string(),
                text: text.to_string(),
            });
            Ok(())
        }

        fn send_message_returning(
            &self,
            chat_id: i64,
            text: &str,
            reply_to_message_id: i64,
        ) -> Result<i64, String> {
            let message_id = {
                let mut state = self.state.lock().unwrap();
                let message_id = state.next_message_id;
                state.next_message_id += 1;
                state.calls.push(Call::SendReturning {
                    chat_id,
                    reply_to: reply_to_message_id,
                    text: text.to_string(),
                });
                message_id
            };
            Ok(message_id)
        }

        fn send_message_with_effect(
            &self,
            chat_id: i64,
            text: &str,
            reply_to_message_id: i64,
            message_effect_id: &str,
        ) -> Result<Message, String> {
            let mut state = self.state.lock().unwrap();
            state.calls.push(Call::Effect {
                chat_id,
                reply_to: reply_to_message_id,
                effect_id: message_effect_id.to_string(),
                text: text.to_string(),
            });
            state
                .effects
                .pop_front()
                .unwrap_or_else(|| Ok(message_with_effect(200, Some(message_effect_id))))
        }

        fn delete_message(&self, chat_id: i64, message_id: i64) -> Result<(), String> {
            self.state.lock().unwrap().calls.push(Call::Delete {
                chat_id,
                message_id,
            });
            Ok(())
        }

        fn set_message_reaction(
            &self,
            chat_id: i64,
            message_id: i64,
            emoji: &str,
        ) -> Result<(), String> {
            self.state.lock().unwrap().calls.push(Call::Reaction {
                chat_id,
                message_id,
                emoji: emoji.to_string(),
            });
            Ok(())
        }

        fn edit_message_text(
            &self,
            chat_id: i64,
            message_id: i64,
            text: &str,
        ) -> Result<(), String> {
            self.state.lock().unwrap().calls.push(Call::Edit {
                chat_id,
                message_id,
                text: text.to_string(),
            });
            Ok(())
        }

        fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String> {
            self.state.lock().unwrap().calls.push(Call::Action {
                chat_id,
                action: action.to_string(),
            });
            Ok(())
        }

        fn download_file(&self, file_id: &str, path: &Path) -> Result<(), String> {
            let download = {
                let mut state = self.state.lock().unwrap();
                state.calls.push(Call::Download {
                    file_id: file_id.to_string(),
                    path: path.to_path_buf(),
                });
                state
                    .downloads
                    .pop_front()
                    .unwrap_or_else(|| Ok(Vec::new()))
            };
            let bytes = download?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            fs::write(path, bytes).map_err(|err| err.to_string())
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_message(
        cfg: &Config,
        tg: &impl TelegramApi,
        store: &SessionStore,
        selections: &RuntimeSelections,
        tx: &mpsc::SyncSender<Job>,
        msg: Message,
    ) -> Result<(), String> {
        super::handle_message(
            cfg,
            tg,
            store,
            selections,
            &CancellationState::default(),
            tx,
            msg,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_command(
        cfg: &Config,
        tg: &impl TelegramApi,
        store: &SessionStore,
        selections: &RuntimeSelections,
        msg: &Message,
        text: &str,
        command: &str,
    ) -> Result<(), String> {
        super::handle_command(
            cfg,
            tg,
            store,
            selections,
            &CancellationState::default(),
            msg,
            text,
            command,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_command_with_codex(
        cfg: &Config,
        codex: &CodexConfig,
        tg: &impl TelegramApi,
        store: &SessionStore,
        selections: &RuntimeSelections,
        msg: &Message,
        text: &str,
        command: &str,
    ) -> Result<(), String> {
        super::handle_command_with_codex(
            cfg,
            codex,
            tg,
            store,
            selections,
            &CancellationState::default(),
            msg,
            text,
            command,
        )
    }

    fn run_job_with_codex(
        cfg: &Config,
        codex: &CodexConfig,
        tg: &impl TelegramApi,
        store: &SessionStore,
        job: Job,
    ) -> Result<(), String> {
        super::run_job_with_codex(cfg, codex, tg, store, &CancellationState::default(), job)
    }

    fn assert_no_auto_rename_telegram(tg: &FakeTelegram) {
        let sent = tg.sent_text();
        assert!(
            !sent.iter().any(|text| {
                text.contains("🏷️ Naming current session")
                    || text.contains("🏷️ Renamed session")
                    || text.contains("⚠️ Failed to rename session")
                    || text.contains("⚠️ Failed to save renamed session")
                    || text.contains("🏷️ No current session to rename")
            }),
            "unexpected auto-rename telegram messages: {sent:?}"
        );
    }

    const ASYNC_RENAME_TEST_TIMEOUT: Duration = Duration::from_secs(10);

    fn assert_session_name_eventually(
        store: &SessionStore,
        key: &SessionKey,
        session_id: &str,
        expected_name: &str,
    ) {
        let deadline = Instant::now() + ASYNC_RENAME_TEST_TIMEOUT;
        loop {
            let state = store.load(key);
            if state.sessions.iter().any(|session| {
                session.id == session_id && session.name.as_deref() == Some(expected_name)
            }) {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "session {session_id} was not renamed to {expected_name}: {:?}",
                    state.sessions
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_file_eventually(path: &Path) {
        let deadline = Instant::now() + ASYNC_RENAME_TEST_TIMEOUT;
        loop {
            if path.exists() {
                return;
            }
            if Instant::now() >= deadline {
                panic!("file was not created: {}", path.display());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_session_name_remains_for(
        store: &SessionStore,
        key: &SessionKey,
        session_id: &str,
        expected_name: &str,
        duration: Duration,
    ) {
        let deadline = Instant::now() + duration;
        loop {
            let state = store.load(key);
            let actual = state
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .and_then(|session| session.name.as_deref());
            assert_eq!(actual, Some(expected_name), "session name changed");
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn stop_cancels_active_and_stale_queued_jobs_for_same_chat() {
        let cancellations = CancellationState::default();
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        let mut queued = job("queued");
        queued.cancel_epoch = cancellation_epoch(&cancellations, &key);
        let active = register_active_cancel(&cancellations, &key);

        assert_eq!(cancel_key(&cancellations, &key), 1);

        assert!(active.load(Ordering::SeqCst));
        assert!(is_job_cancelled(&cancellations, &queued));

        unregister_active_cancel(&cancellations, &key, &active);
        assert_eq!(cancel_key(&cancellations, &key), 0);
    }

    fn seed_session_history(store: &SessionStore, key: &SessionKey) {
        assert!(store
            .save_current_session(key, 0, "cccccccc-oldest")
            .unwrap());
        store.reset(key).unwrap();
        assert!(store
            .save_current_session(key, 1, "bbbbbbbb-previous")
            .unwrap());
        store.reset(key).unwrap();
        assert!(store
            .save_current_session(key, 2, "aaaaaaaa-current")
            .unwrap());
    }

    fn test_config(root: &Path) -> Config {
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            telegram_bots: vec![crate::config::TelegramBotConfig {
                bot_token: "token".to_string(),
                chat_ids: vec![42],
                offset_file: root.join("state/gateway/telegram.offset"),
            }],
            xdg_config_home: root.join("config"),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: root.join("config/gateway/config.json"),
            codex_workdir: root.to_path_buf(),
            models: vec![
                ProviderModel {
                    provider: Provider::Codex,
                    model: "gpt-test".to_string(),
                },
                ProviderModel {
                    provider: Provider::Claude,
                    model: "claude-test".to_string(),
                },
                ProviderModel {
                    provider: Provider::Openrouter,
                    model: "openrouter/test".to_string(),
                },
            ],
            state_dir: root.join("state/gateway"),
            chat_state_dir: root.join("state/gateway/chats"),
            offset_file: root.join("state/gateway/telegram.offset"),
            gateway_log_file: root.join("state/gateway/logs/gateway.log"),
            launchd_target: "gui/0/ai.gateway-test".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(5),
        }
    }

    fn test_codex_config(cfg: &Config, bin: PathBuf) -> CodexConfig {
        CodexConfig {
            bin,
            workdir: cfg.codex_workdir.clone(),
            default_model: cfg.default_provider_model().model.clone(),
        }
    }

    fn message(chat_id: i64, message_id: i64, text: &str) -> Message {
        Message {
            message_id,
            message_thread_id: None,
            effect_id: None,
            reply_to_message: None,
            from: Some(User {
                id: chat_id,
                username: String::new(),
            }),
            chat: Chat {
                id: chat_id,
                kind: "private".to_string(),
                username: String::new(),
            },
            text: text.to_string(),
            caption: String::new(),
            photo: Vec::new(),
            document: None,
        }
    }

    fn callback_query(id: &str, message: Message, data: &str) -> CallbackQuery {
        let chat_id = message.chat.id;
        CallbackQuery {
            id: id.to_string(),
            from: User {
                id: chat_id,
                username: String::new(),
            },
            message: Some(message),
            data: data.to_string(),
        }
    }

    fn job(prompt: &str) -> Job {
        Job {
            bot_token: "token".to_string(),
            chat_id: 42,
            thread_id: None,
            reply_to_message_id: 7,
            prompt: prompt.to_string(),
            _attachment_dir: None,
            image_paths: Vec::new(),
            file_paths: Vec::new(),
            provider_model: ProviderModel {
                provider: Provider::Codex,
                model: "gpt-test".to_string(),
            },
            cancel_epoch: 0,
        }
    }

    fn message_with_effect(message_id: i64, effect_id: Option<&str>) -> Message {
        Message {
            message_id,
            message_thread_id: None,
            effect_id: effect_id.map(ToOwned::to_owned),
            reply_to_message: None,
            from: None,
            chat: Chat {
                id: 42,
                kind: "private".to_string(),
                username: String::new(),
            },
            text: String::new(),
            caption: String::new(),
            photo: Vec::new(),
            document: None,
        }
    }

    fn executable(path: PathBuf, body: &str) -> PathBuf {
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    struct MiniServer {
        base_url: String,
        _handle: JoinHandle<()>,
    }

    impl MiniServer {
        fn new(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}/botsecret", listener.local_addr().unwrap());
            let handle = thread::spawn(move || {
                for response in responses {
                    let (stream, _) = listener.accept().unwrap();
                    drain_request_and_respond(stream, &response);
                }
            });
            Self {
                base_url,
                _handle: handle,
            }
        }
    }

    fn drain_request_and_respond(mut stream: TcpStream, response: &str) {
        let mut content_length = 0;
        {
            let mut reader = BufReader::new(&mut stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
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
        }
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn json_response(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn telegram_message_json(message_id: i64, effect_id: Option<&str>) -> String {
        let effect = effect_id
            .map(|id| format!(r#","effect_id":"{id}""#))
            .unwrap_or_default();
        format!(
            r#"{{"ok":true,"result":{{"message_id":{message_id}{effect},"from":null,"chat":{{"id":42,"type":"private"}},"text":"sent","caption":""}}}}"#
        )
    }
}
