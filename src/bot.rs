use crate::codex::{run_codex, run_codex_stream, CodexConfig, CodexRun};
use crate::commands::{directive_help, is_allowed, unknown_directive_message};
use crate::config::{Config, ProviderModel};
use crate::provider::Provider;
use crate::session::{SessionKey, SessionStore};
use crate::status::{codex_status, fastfetch_status, format_status_message};
use crate::telegram::{CallbackQuery, InlineKeyboardButton, Message, TelegramClient, Update};
use crate::text::{
    command_arg, is_ok_response, log_line_count, parse_command, redact_private_data, session_label,
    split_telegram_message, tail_log_text,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TYPING_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const POLL_CONFLICT_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const TELEGRAM_GET_UPDATES_CONFLICT_MARKER: &str = "terminated by other getUpdates request";

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
}

#[derive(Debug, Clone)]
struct Job {
    chat_id: i64,
    thread_id: Option<i64>,
    reply_to_message_id: i64,
    prompt: String,
    provider_model: ProviderModel,
}

type RuntimeSelections = Arc<Mutex<HashMap<SessionKey, ProviderModel>>>;

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
    let tg = TelegramClient::new(&cfg.bot_token);
    run_with_client(cfg, tg)
}

fn run_with_client<T: TelegramApi>(cfg: Config, tg: T) -> Result<(), String> {
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    fs::create_dir_all(&cfg.chat_state_dir)
        .map_err(|err| format!("create chat state dir: {err}"))?;

    if let Err(err) = tg.sync_my_commands(&cfg.telegram_chat_ids) {
        eprintln!("telegram command sync failed; continuing without command refresh: {err}");
    }
    let default_model = cfg.default_provider_model().clone();
    let store = SessionStore::new_with_provider(
        cfg.chat_state_dir.clone(),
        default_model.model.clone(),
        default_model.provider,
    );
    let status_codex = codex_status(&cfg);
    let status_fetch = fastfetch_status(&cfg.fastfetch_bin);
    for chat_id in &cfg.telegram_chat_ids {
        let state = store.load(&SessionKey::Chat {
            chat_id: *chat_id,
            thread_id: None,
        });
        if let Err(err) = send_long_message(
            &tg,
            *chat_id,
            &format_status_message(&state, &status_codex, &status_fetch),
            0,
        ) {
            eprintln!("telegram startup status send failed for chat {chat_id}: {err}");
        }
    }

    let (tx, rx) = mpsc::sync_channel::<Job>(cfg.queue_depth);
    let worker_cfg = cfg.clone();
    let _worker = thread::spawn(move || worker_loop(worker_cfg, rx));
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
                eprintln!(
                    "[gateway] telegram getUpdates conflict; another bot instance is polling this token; retrying in {}s: {err}",
                    POLL_CONFLICT_RETRY_INTERVAL.as_secs()
                );
                sleep_after_get_updates_conflict();
                continue;
            }
            Err(err) => return Err(err),
        };
        for update in updates {
            offset = advance_offset(offset, update.update_id);
            write_offset(&cfg.offset_file, offset)?;
            if let Some(message) = update.message {
                if let Err(err) = handle_message(&cfg, &tg, &store, &selections, &tx, message) {
                    eprintln!("[gateway] message handler failed: {err}");
                }
            }
            if let Some(callback_query) = update.callback_query {
                if let Err(err) =
                    handle_callback_query(&cfg, &tg, &store, &selections, callback_query)
                {
                    eprintln!("[gateway] callback handler failed: {err}");
                }
            }
        }
    }
}

fn is_get_updates_conflict(err: &str) -> bool {
    err.starts_with("Conflict:") && err.contains(TELEGRAM_GET_UPDATES_CONFLICT_MARKER)
}

fn sleep_after_get_updates_conflict() {
    #[cfg(not(test))]
    thread::sleep(POLL_CONFLICT_RETRY_INTERVAL);
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
    tx: &mpsc::SyncSender<Job>,
    msg: Message,
) -> Result<(), String> {
    if !is_allowed_private_message(&cfg.telegram_chat_ids, &msg) {
        return Ok(());
    }
    let text = match message_text(&msg.text, &msg.caption) {
        Ok(text) => text,
        Err(err) => {
            tg.send_message(msg.chat.id, &err, msg.message_id)?;
            return Ok(());
        }
    };

    if let Some(command) = parse_command(&text) {
        return handle_command(cfg, tg, store, selections, &msg, &text, &command);
    }

    let queued = tx.try_send(Job {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
        reply_to_message_id: msg.message_id,
        prompt: text,
        provider_model: selected_provider_model(
            cfg,
            selections,
            &SessionKey::Chat {
                chat_id: msg.chat.id,
                thread_id: msg.message_thread_id,
            },
        ),
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

fn handle_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    text: &str,
    command: &str,
) -> Result<(), String> {
    let key = SessionKey::Chat {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
    };
    match command {
        "/log" => handle_log_command(cfg, tg, msg, text),
        "/new" => handle_new_command(cfg, tg, store, selections, msg, &key),
        "/restart" => {
            tg.send_message(msg.chat.id, "🔄 Restarting gateway.", msg.message_id)?;
            restart_gateway(&cfg.launchd_target);
            Ok(())
        }
        "/model" => handle_model_command(cfg, tg, store, selections, msg, text, &key),
        "/resume" => handle_resume_command(tg, store, selections, msg, text, &key),
        "/rename" => handle_rename_command(cfg, tg, store, msg, text, &key),
        "/list" => send_long_message(tg, msg.chat.id, &store.list(&key), msg.message_id),
        "/help" => tg.send_message(msg.chat.id, &directive_help(), msg.message_id),
        "/status" => handle_status_command(cfg, tg, store, selections, msg, &key),
        _ => tg.send_message(msg.chat.id, &unknown_directive_message(), msg.message_id),
    }
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
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    let state = store.load(key);
    if current_session_is_unnamed(&state) && !auto_rename_current_session(cfg, tg, store, msg, key)?
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
        |back| store.resume_relative(key, back),
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
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    text: &str,
    key: &SessionKey,
) -> Result<(), String> {
    let name = command_arg(text);
    if name.is_empty() {
        return handle_auto_rename_command(cfg, tg, store, msg, key);
    }
    rename_session(tg, store, msg, key, &name)
}

fn handle_auto_rename_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    auto_rename_current_session(cfg, tg, store, msg, key).map(|_| ())
}

fn auto_rename_current_session(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
) -> Result<bool, String> {
    let state = store.load(key);
    if state.session_id.is_none() {
        return tg
            .send_message(
                msg.chat.id,
                "🏷️ No current session to rename. Send a normal message first.",
                msg.message_id,
            )
            .map(|_| false);
    }
    tg.send_message(msg.chat.id, "🏷️ Naming current session…", msg.message_id)?;
    let output = match run_codex(
        &CodexConfig::from(cfg),
        AUTO_RENAME_PROMPT,
        state.session_id.as_deref(),
        Provider::Codex,
        AUTO_RENAME_MODEL,
        cfg.codex_timeout,
        &cfg.state_dir,
    ) {
        Ok(output) => output,
        Err(err) => {
            return tg
                .send_message(
                    msg.chat.id,
                    &format!("⚠️ Failed to rename session: {err}"),
                    msg.message_id,
                )
                .map(|_| false)
        }
    };
    let Some(name) = auto_session_name(&output.final_text) else {
        return tg
            .send_message(
                msg.chat.id,
                "⚠️ Failed to rename session: Codex returned an invalid session name.",
                msg.message_id,
            )
            .map(|_| false);
    };
    if let Some(session_id) = output.session_id.as_deref() {
        if let Err(err) = store.save_current_session(key, state.generation, session_id) {
            return tg
                .send_message(
                    msg.chat.id,
                    &format!("⚠️ Failed to save renamed session: {err}"),
                    msg.message_id,
                )
                .map(|_| false);
        }
    }
    rename_current_session(tg, store, msg, key, &name)
}

fn rename_current_session(
    tg: &impl TelegramApi,
    store: &SessionStore,
    msg: &Message,
    key: &SessionKey,
    name: &str,
) -> Result<bool, String> {
    match store.rename_current(key, name) {
        Ok(state) => tg
            .send_message(
                msg.chat.id,
                &format!(
                    "🏷️ Renamed session {} to \"{name}\".",
                    session_label(state.session_id.as_deref().unwrap_or(""))
                ),
                msg.message_id,
            )
            .map(|_| true),
        Err(err) => tg
            .send_message(msg.chat.id, &err, msg.message_id)
            .map(|_| false),
    }
}

const AUTO_RENAME_MODEL: &str = "gpt-5.4-mini";
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
    match store.rename_current(key, name) {
        Ok(state) => tg.send_message(
            msg.chat.id,
            &format!(
                "🏷️ Renamed session {} to \"{name}\".",
                session_label(state.session_id.as_deref().unwrap_or(""))
            ),
            msg.message_id,
        ),
        Err(err) => tg.send_message(msg.chat.id, &err, msg.message_id),
    }
}

fn handle_status_command(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    msg: &Message,
    key: &SessionKey,
) -> Result<(), String> {
    let state = store.load(key);
    send_long_message(
        tg,
        msg.chat.id,
        &format_status_message(
            &state_with_provider_model(&state, &selected_provider_model(cfg, selections, key)),
            &codex_status(cfg),
            &fastfetch_status(&cfg.fastfetch_bin),
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

fn worker_loop(cfg: Config, rx: mpsc::Receiver<Job>) {
    let tg = TelegramClient::new(&cfg.bot_token);
    let default_model = cfg.default_provider_model().clone();
    let store = SessionStore::new_with_provider(
        cfg.chat_state_dir.clone(),
        default_model.model,
        default_model.provider,
    );
    for job in rx {
        if let Err(err) = run_job(&cfg, &tg, &store, job) {
            eprintln!("[gateway] job handler failed: {err}");
        }
    }
}

fn run_job(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    job: Job,
) -> Result<(), String> {
    let started = Instant::now();
    let key = SessionKey::Chat {
        chat_id: job.chat_id,
        thread_id: job.thread_id,
    };
    let state = store.load(&key);
    eprintln!(
        "[gateway] job start chat={} reply_to={} provider={} model={} session={} prompt_chars={} timeout_secs={}",
        job.chat_id,
        job.reply_to_message_id,
        job.provider_model.provider.label(),
        job.provider_model.model,
        session_label(state.session_id.as_deref().unwrap_or("")),
        job.prompt.chars().count(),
        cfg.codex_timeout.as_secs()
    );
    let stream_message_id =
        tg.send_message_returning(job.chat_id, "🫧 Thinking…", job.reply_to_message_id)?;
    let mut streamed = String::new();
    let mut last_edit = Instant::now();
    let run_result = {
        let _typing = start_typing_loop(tg, job.chat_id);
        run_codex_stream(
            &CodexConfig::from(cfg),
            CodexRun {
                prompt: &job.prompt,
                session_id: state.session_id.as_deref(),
                provider: job.provider_model.provider,
                model: &job.provider_model.model,
                timeout: cfg.codex_timeout,
                state_dir: &cfg.state_dir,
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
            eprintln!(
                "[gateway] job provider failed chat={} provider={} elapsed_ms={} error={}",
                job.chat_id,
                job.provider_model.provider.label(),
                started.elapsed().as_millis(),
                single_line(&err)
            );
            send_long_message(
                tg,
                job.chat_id,
                &format!("⚠️ {} failed:\n{err}", job.provider_model.provider.label()),
                job.reply_to_message_id,
            )?;
            return Ok(());
        }
    };
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_current_session(&key, state.generation, session_id)?;
    }
    eprintln!(
        "[gateway] job success chat={} elapsed_ms={} final_chars={} session={}",
        job.chat_id,
        started.elapsed().as_millis(),
        output.final_text.chars().count(),
        session_label(output.session_id.as_deref().unwrap_or(""))
    );
    match final_delivery(&output.final_text) {
        FinalDelivery::Reaction => {
            let _ = tg.delete_message(job.chat_id, stream_message_id);
            tg.set_message_reaction(job.chat_id, job.reply_to_message_id, "👍")
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
                eprintln!(
                    "[gateway] telegram final effect missing chat={} message={} requested_effect={} returned_effect={}",
                    job.chat_id,
                    message.message_id,
                    FINAL_MESSAGE_EFFECT_ID,
                    message.effect_id.as_deref().unwrap_or("<none>")
                );
            }
            Ok(())
        }
        Err(err) => {
            eprintln!(
                "[gateway] telegram final effect failed chat={} effect={} error={}",
                job.chat_id, FINAL_MESSAGE_EFFECT_ID, err
            );
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

fn restart_gateway(launchd_target: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", launchd_target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub const fn typing_refresh_interval() -> Duration {
    TYPING_REFRESH_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telegram::{Chat, User};
    use std::collections::VecDeque;
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
        assert!(tg.sent_text().iter().any(|text| text.contains("/status")));
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
        handle_command(&cfg, &tg, &store, &selections, &msg, "/rename", "/rename").unwrap();
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
        handle_command(&cfg, &tg, &store, &selections, &msg, "/list", "/list").unwrap();
        handle_command(&cfg, &tg, &store, &selections, &msg, "/help", "/help").unwrap();
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
        assert!(sent
            .iter()
            .any(|text| text.contains("🏷️ Naming current session")));
        assert!(sent
            .iter()
            .any(|text| text.contains("invalid session name")));
        assert!(sent.iter().any(|text| text.contains("🏷️ Renamed session")));
        assert!(sent.iter().any(|text| text.contains("💾 Saved sessions:")));
        assert!(sent.iter().any(|text| text.contains("/status")));
        assert!(!sent.iter().any(|text| text.contains("/commands")));
        assert_eq!(
            sent.iter()
                .filter(|text| text.contains("❓ Unknown directive"))
                .count(),
            2
        );
        assert!(sent.iter().any(|text| text.contains("🧠 Codex:")));
        assert!(sent.iter().any(|text| text == "🔄 Restarting gateway."));
        assert!(sent
            .iter()
            .any(|text| text.contains("❓ Unknown directive")));
    }

    #[test]
    fn resume_numeric_argument_steps_back_from_current_session() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        let selections = RuntimeSelections::default();
        let msg = message(42, 10, "/resume 1");
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };
        seed_session_history(&store, &key);

        handle_command(&cfg, &tg, &store, &selections, &msg, "/resume 1", "/resume").unwrap();

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
    fn rename_without_name_asks_codex_for_session_name() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
            dir.path().join("codex-title"),
            r#"#!/bin/sh
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

        handle_command(&cfg, &tg, &store, &selections, &msg, "/rename", "/rename").unwrap();

        let state = store.load(&key);
        assert_eq!(state.sessions[0].name.as_deref(), Some("session-name"));
        let args = fs::read_to_string(dir.path().join("codex-title.args")).unwrap();
        assert!(args
            .lines()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["-m", "gpt-5.4-mini"]));
        let prompt = fs::read_to_string(dir.path().join("codex-title.prompt")).unwrap();
        assert!(prompt.contains("lowercase single-word"));
        assert!(prompt.contains("session-name"));
        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text.contains("🏷️ Renamed session aaaaaaaa to \"session-name\".")));
    }

    #[test]
    fn rename_without_name_rejects_invalid_codex_session_name() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
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
"#,
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

        handle_command(&cfg, &tg, &store, &selections, &msg, "/rename", "/rename").unwrap();

        let state = store.load(&key);
        assert_eq!(state.sessions[0].name, None);
        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text.contains("invalid session name")));
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
    fn new_auto_renames_current_unnamed_session_before_reset() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
            dir.path().join("codex-title-new"),
            r#"#!/bin/sh
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

        handle_command(&cfg, &tg, &store, &selections, &msg, "/new", "/new").unwrap();

        let state = store.load(&key);
        assert_eq!(state.session_id, None);
        let saved = state
            .sessions
            .iter()
            .find(|session| session.id == "aaaaaaaa-current")
            .unwrap();
        assert_eq!(saved.name.as_deref(), Some("session-name"));
        let sent = tg.sent_text();
        assert!(sent
            .iter()
            .any(|text| text.contains("🏷️ Naming current session")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🏷️ Renamed session aaaaaaaa to \"session-name\".")));
        assert!(sent
            .iter()
            .any(|text| text.contains("🆕 New session ready")));
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
        assert!(sent.iter().any(|text| text.contains("No current session")));
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
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
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
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job(&cfg, &tg, &store, job("make it so")).unwrap();

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
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
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
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.push_effect(Ok(message_with_effect(200, None)));

        run_job(&cfg, &tg, &store, job("answer")).unwrap();

        assert!(tg.calls().contains(&Call::Effect {
            chat_id: 42,
            reply_to: 7,
            effect_id: FINAL_MESSAGE_EFFECT_ID.to_string(),
            text: "final text".to_string()
        }));

        let tg = FakeTelegram::new();
        tg.push_effect(Err("effect failed".to_string()));
        run_job(&cfg, &tg, &store, job("answer")).unwrap();

        assert!(tg.sent_text().contains(&"final text".to_string()));
    }

    #[test]
    fn run_job_redacts_final_text_before_sending_to_telegram() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
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
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job(&cfg, &tg, &store, job("answer")).unwrap();

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
        let mut cfg = test_config(dir.path());
        let final_text = "a".repeat(crate::text::TELEGRAM_MESSAGE_LIMIT + 20);
        cfg.codex_bin = executable(
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
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job(&cfg, &tg, &store, job("stream")).unwrap();

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
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
            dir.path().join("codex-fails"),
            r#"#!/bin/sh
printf 'boom\n' >&2
exit 2
"#,
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();

        run_job(&cfg, &tg, &store, job("fail")).unwrap();

        assert!(tg
            .sent_text()
            .iter()
            .any(|text| text.contains("⚠️ Codex failed:\nboom")));
    }

    #[test]
    fn run_job_propagates_codex_failure_delivery_errors() {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
            dir.path().join("codex-fails"),
            r#"#!/bin/sh
printf 'boom\n' >&2
exit 2
"#,
        );
        let store = SessionStore::new(
            cfg.chat_state_dir.clone(),
            cfg.default_provider_model().model.clone(),
        );
        let tg = FakeTelegram::new();
        tg.fail_sends("send failed");

        let err = run_job(&cfg, &tg, &store, job("fail")).unwrap_err();

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
        let codex = executable(
            root.join("codex"),
            r#"#!/bin/sh
if [ "$1" = "doctor" ]; then
  printf '{"overallStatus":"ok","codexVersion":"test","checks":{"auth.credentials":{"status":"ok"},"network.provider_reachability":{"status":"ok"}}}\n'
  exit 0
fi
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'OK\n' > "$out"
"#,
        );
        let fastfetch = executable(
            root.join("fastfetch"),
            r#"#!/bin/sh
cat >/dev/null
printf 'OS: test\n'
"#,
        );
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            xdg_config_home: root.join("config"),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: root.join("config/gateway/config.json"),
            codex_bin: codex,
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
            fastfetch_bin: fastfetch,
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

    fn message(chat_id: i64, message_id: i64, text: &str) -> Message {
        Message {
            message_id,
            message_thread_id: None,
            effect_id: None,
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
            chat_id: 42,
            thread_id: None,
            reply_to_message_id: 7,
            prompt: prompt.to_string(),
            provider_model: ProviderModel {
                provider: Provider::Codex,
                model: "gpt-test".to_string(),
            },
        }
    }

    fn message_with_effect(message_id: i64, effect_id: Option<&str>) -> Message {
        Message {
            message_id,
            message_thread_id: None,
            effect_id: effect_id.map(ToOwned::to_owned),
            from: None,
            chat: Chat {
                id: 42,
                kind: "private".to_string(),
                username: String::new(),
            },
            text: String::new(),
            caption: String::new(),
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
