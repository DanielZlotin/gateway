use crate::codex::{run_codex_stream, CodexConfig};
use crate::commands::{directive_help, is_allowed, unknown_directive_message};
use crate::config::{save_gateway_config, Config, GatewayConfigFile};
use crate::session::{SessionKey, SessionStore};
use crate::status::{fastfetch_status, format_status_message, status_header};
use crate::telegram::{Message, TelegramClient, Update};
use crate::text::{
    command_arg, log_line_count, parse_command, session_label, split_telegram_message,
    tail_log_text,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const TYPING_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
struct Job {
    chat_id: i64,
    thread_id: Option<i64>,
    reply_to_message_id: i64,
    prompt: String,
}

struct TypingLoop {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
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
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    fs::create_dir_all(&cfg.chat_state_dir)
        .map_err(|err| format!("create chat state dir: {err}"))?;
    fs::create_dir_all(&cfg.cron_state_dir)
        .map_err(|err| format!("create cron state dir: {err}"))?;

    let tg = TelegramClient::new(&cfg.bot_token);
    tg.sync_my_commands(&cfg.allowed_ids)?;
    let store = SessionStore::new(
        cfg.chat_state_dir.clone(),
        cfg.cron_state_dir.clone(),
        cfg.codex_model.clone(),
    );
    for chat_id in &cfg.allowed_ids {
        let state = store.load(&SessionKey::Chat {
            chat_id: *chat_id,
            thread_id: None,
        });
        send_long_message(
            &tg,
            *chat_id,
            &format_status_message(&state, &fastfetch_status(&cfg.fastfetch_bin)),
            0,
        )?;
    }

    let (tx, rx) = mpsc::sync_channel::<Job>(cfg.queue_depth);
    let worker_cfg = cfg.clone();
    let _worker = thread::spawn(move || worker_loop(worker_cfg, rx));

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
        let updates = tg.get_updates(offset, cfg.poll_timeout_sec)?;
        for update in updates {
            offset = advance_offset(offset, update.update_id);
            write_offset(&cfg.offset_file, offset)?;
            if let Some(message) = update.message {
                handle_message(&cfg, &tg, &store, &tx, message)?;
            }
        }
    }
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
    Err("Text messages only.".to_string())
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
    tg: &TelegramClient,
    store: &SessionStore,
    tx: &mpsc::SyncSender<Job>,
    msg: Message,
) -> Result<(), String> {
    let from_id = msg.from.as_ref().map(|user| user.id);
    if !is_allowed(&cfg.allowed_ids, msg.chat.id, from_id) {
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
        return handle_command(cfg, tg, store, &msg, &text, &command);
    }

    let queued = tx.try_send(Job {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
        reply_to_message_id: msg.message_id,
        prompt: text,
    });
    if queued.is_err() {
        tg.send_message(
            msg.chat.id,
            "Codex queue is full. Try again after the current requests finish.",
            msg.message_id,
        )?;
    } else {
        let _ = tg.send_chat_action(msg.chat.id, "typing");
    }
    Ok(())
}

fn handle_command(
    cfg: &Config,
    tg: &TelegramClient,
    store: &SessionStore,
    msg: &Message,
    text: &str,
    command: &str,
) -> Result<(), String> {
    let key = SessionKey::Chat {
        chat_id: msg.chat.id,
        thread_id: msg.message_thread_id,
    };
    match command {
        "/log" => {
            let lines = log_line_count(text);
            let body = fs::read_to_string(&cfg.gateway_log_file)
                .map(|log_text| tail_log_text(&log_text, lines))
                .unwrap_or_else(|_| "No gateway log available.".to_string());
            send_long_message(tg, msg.chat.id, &body, msg.message_id)
        }
        "/new" => match store.reset(&key) {
            Ok(state) => tg.send_message(
                msg.chat.id,
                &format!("New session ready. Model: {}", state.model),
                msg.message_id,
            ),
            Err(err) => tg.send_message(
                msg.chat.id,
                &format!("Failed to reset session: {err}"),
                msg.message_id,
            ),
        },
        "/restart" => {
            tg.send_message(msg.chat.id, "Restarting gateway.", msg.message_id)?;
            restart_gateway(&cfg.launchd_target);
            Ok(())
        }
        "/model" => {
            let model = command_arg(text);
            if model.is_empty() {
                let state = store.load(&key);
                return tg.send_message(msg.chat.id, &status_header(&state), msg.message_id);
            }
            match store.set_model(&key, &model) {
                Ok(state) => {
                    save_gateway_config(
                        &cfg.gateway_config_file,
                        &GatewayConfigFile {
                            model: state.model.clone(),
                        },
                    )?;
                    tg.send_message(
                        msg.chat.id,
                        &format!(
                            "Model set to {}\nSession: {}",
                            state.model,
                            session_label(state.session_id.as_deref().unwrap_or(""))
                        ),
                        msg.message_id,
                    )
                }
                Err(err) => tg.send_message(
                    msg.chat.id,
                    &format!("Failed to set model: {err}"),
                    msg.message_id,
                ),
            }
        }
        "/resume" => {
            let target = command_arg(text);
            if target.is_empty() {
                let body = format!("Usage: /resume SESSION_OR_NAME\n\n{}", store.list(&key));
                return send_long_message(tg, msg.chat.id, &body, msg.message_id);
            }
            match store.resume(&key, &target) {
                Ok(state) => tg.send_message(
                    msg.chat.id,
                    &format!(
                        "Resumed session {}\nModel: {}",
                        session_label(state.session_id.as_deref().unwrap_or("")),
                        state.model
                    ),
                    msg.message_id,
                ),
                Err(err) => tg.send_message(msg.chat.id, &err, msg.message_id),
            }
        }
        "/rename" => {
            let name = command_arg(text);
            if name.is_empty() {
                return tg.send_message(msg.chat.id, "Usage: /rename NAME", msg.message_id);
            }
            match store.rename_current(&key, &name) {
                Ok(state) => tg.send_message(
                    msg.chat.id,
                    &format!(
                        "Renamed session {} to \"{name}\".",
                        session_label(state.session_id.as_deref().unwrap_or(""))
                    ),
                    msg.message_id,
                ),
                Err(err) => tg.send_message(msg.chat.id, &err, msg.message_id),
            }
        }
        "/list" => send_long_message(tg, msg.chat.id, &store.list(&key), msg.message_id),
        "/help" | "/commands" => tg.send_message(msg.chat.id, &directive_help(), msg.message_id),
        "/status" => {
            let state = store.load(&key);
            send_long_message(
                tg,
                msg.chat.id,
                &format_status_message(&state, &fastfetch_status(&cfg.fastfetch_bin)),
                msg.message_id,
            )
        }
        _ => tg.send_message(msg.chat.id, &unknown_directive_message(), msg.message_id),
    }
}

fn worker_loop(cfg: Config, rx: mpsc::Receiver<Job>) {
    let tg = TelegramClient::new(&cfg.bot_token);
    let store = SessionStore::new(
        cfg.chat_state_dir.clone(),
        cfg.cron_state_dir.clone(),
        cfg.codex_model.clone(),
    );
    for job in rx {
        if let Err(err) = run_job(&cfg, &tg, &store, job) {
            eprintln!("[gateway] job handler failed: {err}");
        }
    }
}

fn run_job(
    cfg: &Config,
    tg: &TelegramClient,
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
        "[gateway] job start chat={} reply_to={} model={} session={} prompt_chars={} timeout_secs={}",
        job.chat_id,
        job.reply_to_message_id,
        state.model,
        session_label(state.session_id.as_deref().unwrap_or("")),
        job.prompt.chars().count(),
        cfg.codex_timeout.as_secs()
    );
    let stream_message_id = tg.send_message_returning(
        job.chat_id,
        "⏳ Codex is starting…",
        job.reply_to_message_id,
    )?;
    let mut streamed = String::new();
    let mut last_edit = Instant::now();
    let output = match {
        let _typing = start_typing_loop(tg, job.chat_id);
        run_codex_stream(
            &CodexConfig {
                bin: cfg.codex_bin.clone(),
                home: cfg.codex_home.clone(),
                user_home: cfg.user_home.clone(),
                xdg_config_home: cfg.xdg_config_home.clone(),
                xdg_cache_home: cfg.xdg_cache_home.clone(),
                xdg_data_home: cfg.xdg_data_home.clone(),
                xdg_state_home: cfg.xdg_state_home.clone(),
                workdir: cfg.codex_workdir.clone(),
                path: cfg.path.clone(),
                default_model: cfg.codex_model.clone(),
                instructions_file: cfg.state_dir.join("AGENTS.md"),
            },
            &job.prompt,
            state.session_id.as_deref(),
            &state.model,
            cfg.codex_timeout,
            &cfg.state_dir,
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
    } {
        Ok(output) => output,
        Err(err) => {
            eprintln!(
                "[gateway] job codex failed chat={} elapsed_ms={} error={}",
                job.chat_id,
                started.elapsed().as_millis(),
                single_line(&err)
            );
            send_long_message(
                tg,
                job.chat_id,
                &format!("Codex failed:\n{err}"),
                job.reply_to_message_id,
            )?;
            return Ok(());
        }
    };
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_run(&key, state.generation, session_id)?;
    }
    eprintln!(
        "[gateway] job success chat={} elapsed_ms={} final_chars={} session={}",
        job.chat_id,
        started.elapsed().as_millis(),
        output.final_text.chars().count(),
        session_label(output.session_id.as_deref().unwrap_or(""))
    );
    let final_text = empty_final_text(&output.final_text);
    let parts = split_telegram_message(&final_text);
    if let Some(first) = parts.first() {
        let _ = tg.edit_message_text(job.chat_id, stream_message_id, first);
        for part in parts.iter().skip(1) {
            tg.send_message(job.chat_id, part, 0)?;
        }
        Ok(())
    } else {
        Ok(())
    }
}

fn single_line(text: &str) -> String {
    text.lines().collect::<Vec<_>>().join(" | ")
}

fn start_typing_loop(tg: &TelegramClient, chat_id: i64) -> TypingLoop {
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
        "Codex finished with no final text.".to_string()
    } else {
        text.to_string()
    }
}

fn stream_preview(text: &str) -> String {
    let text = text.trim();
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
    tg: &TelegramClient,
    chat_id: i64,
    text: &str,
    reply_to_message_id: i64,
) -> Result<(), String> {
    for (index, part) in split_telegram_message(text).into_iter().enumerate() {
        let reply = if index == 0 { reply_to_message_id } else { 0 };
        tg.send_message(chat_id, &part, reply)?;
    }
    Ok(())
}

fn restart_gateway(launchd_target: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", launchd_target])
        .spawn();
}

pub fn typing_refresh_interval() -> Duration {
    TYPING_REFRESH_INTERVAL
}

#[cfg(test)]
mod tests {
    use super::*;
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
            },
            crate::telegram::Update {
                update_id: 9,
                message: None,
            },
        ];

        assert_eq!(skip_offset(&updates), 10);
    }

    #[test]
    fn message_text_prefers_text_then_caption() {
        assert_eq!(message_text(" hello ", "caption").unwrap(), "hello");
        assert_eq!(message_text("", " caption ").unwrap(), "caption");
        assert_eq!(message_text("", "").unwrap_err(), "Text messages only.");
    }

    #[test]
    fn typing_refreshes_before_telegram_expires() {
        assert!(typing_refresh_interval() < Duration::from_secs(5));
    }
}
