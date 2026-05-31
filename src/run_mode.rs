use crate::cli::RunArgs;
use crate::codex::{run_codex, CodexConfig};
use crate::config::Config;
use crate::session::{SessionKey, SessionStore};
use crate::telegram::TelegramClient;
use crate::text::{is_ok_response, split_telegram_message};
use std::fs;
use std::io::Read;

pub fn load_prompt<R: Read>(args: &RunArgs, mut stdin: R) -> Result<String, String> {
    let prompt = match (&args.prompt, &args.prompt_file) {
        (Some(prompt), _) => prompt.clone(),
        (None, Some(path)) => {
            fs::read_to_string(path).map_err(|err| format!("read prompt file: {err}"))?
        }
        (None, None) => {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|err| format!("read stdin: {err}"))?;
            text
        }
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("prompt is empty".to_string());
    }
    Ok(prompt)
}

fn should_send_telegram_result(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && !is_ok_response(text)
}

pub fn run(args: RunArgs, cfg: Config) -> Result<String, String> {
    run_with_sender(args, cfg, |bot_token, chat_id, text| {
        let tg = TelegramClient::new(bot_token);
        tg.send_message(chat_id, text, 0)
    })
}

fn run_with_sender(
    args: RunArgs,
    cfg: Config,
    mut send_telegram: impl FnMut(&str, i64, &str) -> Result<(), String>,
) -> Result<String, String> {
    let prompt = load_prompt(&args, std::io::stdin())?;
    let store = SessionStore::new(
        cfg.chat_state_dir.clone(),
        cfg.cron_state_dir.clone(),
        cfg.codex_model.clone(),
    );
    let key = SessionKey::Cron(args.job.clone());
    if args.new_session {
        store.reset(&key)?;
    }
    let state = store.load(&key);
    let model = args.model.as_deref().unwrap_or(&state.model);
    let output = run_codex(
        &CodexConfig::from(&cfg),
        &prompt,
        state.session_id.as_deref(),
        model,
        cfg.codex_timeout,
        &cfg.state_dir,
    )?;
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_run(&key, state.generation, session_id)?;
    }
    if should_send_telegram_result(&output.final_text) {
        for chat_id in &cfg.telegram_chat_ids {
            for part in split_telegram_message(&output.final_text) {
                send_telegram(&cfg.bot_token, *chat_id, &part)?;
            }
        }
    }
    Ok(output.final_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::io::Cursor;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;

    fn base_args() -> RunArgs {
        RunArgs {
            job: "daily".to_string(),
            prompt: None,
            prompt_file: None,
            model: None,
            new_session: false,
        }
    }

    #[test]
    fn prompt_argument_wins_over_stdin() {
        let mut args = base_args();
        args.prompt = Some("from arg".to_string());

        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from arg"
        );
    }

    #[test]
    fn prompt_file_wins_over_stdin() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "from file").unwrap();
        let mut args = base_args();
        args.prompt_file = Some(file.path().to_path_buf());

        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from file"
        );
    }

    #[test]
    fn stdin_is_used_when_no_prompt_source_is_given() {
        let args = base_args();
        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from stdin"
        );
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let args = base_args();
        let err = load_prompt(&args, Cursor::new("   ")).unwrap_err();
        assert!(err.contains("prompt is empty"));
    }

    #[test]
    fn run_mode_sends_nothing_to_telegram_for_empty_or_ok_results() {
        assert!(!should_send_telegram_result(""));
        assert!(!should_send_telegram_result(" \n\t "));
        assert!(!should_send_telegram_result("OK"));
        assert!(!should_send_telegram_result(" OK\n"));
        assert!(!should_send_telegram_result("ok"));
        assert!(!should_send_telegram_result(" oK\n"));
        assert!(should_send_telegram_result("done"));
    }

    #[test]
    fn run_mode_returns_ok_and_sends_no_telegram_message() {
        let dir = tempfile::tempdir().unwrap();
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
printf 'session id: session-cli\n' >&2
"#,
        );
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let mut args = base_args();
        args.prompt = Some("finish quietly".to_string());

        let output = run_with_sender(args, cfg.clone(), move |token, chat_id, text| {
            sent.lock()
                .unwrap()
                .push((token.to_string(), chat_id, text.to_string()));
            Ok(())
        })
        .unwrap();

        assert_eq!(output, "OK");
        assert!(sends.lock().unwrap().is_empty());
        let store = SessionStore::new(
            cfg.chat_state_dir,
            cfg.cron_state_dir,
            cfg.codex_model.clone(),
        );
        let state = store.load(&SessionKey::Cron("daily".to_string()));
        assert_eq!(state.session_id.as_deref(), Some("session-cli"));
    }

    #[test]
    fn run_mode_resets_saves_and_sends_non_ok_results() {
        let dir = tempfile::tempdir().unwrap();
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
printf 'done\n' > "$out"
printf 'session id: session-run\n' >&2
"#,
        );
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let args = RunArgs {
            job: "daily".to_string(),
            prompt: Some("work".to_string()),
            prompt_file: None,
            model: Some("gpt-override".to_string()),
            new_session: true,
        };

        let output = run_with_sender(args, cfg.clone(), move |token, chat_id, text| {
            sent.lock()
                .unwrap()
                .push((token.to_string(), chat_id, text.to_string()));
            Ok(())
        })
        .unwrap();

        assert_eq!(output, "done");
        assert_eq!(
            *sends.lock().unwrap(),
            vec![("token".to_string(), 42, "done".to_string())]
        );
        let store = SessionStore::new(
            cfg.chat_state_dir,
            cfg.cron_state_dir,
            cfg.codex_model.clone(),
        );
        let state = store.load(&SessionKey::Cron("daily".to_string()));
        assert_eq!(state.session_id.as_deref(), Some("session-run"));
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn run_mode_propagates_codex_and_telegram_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.codex_bin = executable(
            dir.path().join("codex-fails"),
            r#"#!/bin/sh
printf 'codex failed\n' >&2
exit 2
"#,
        );
        let mut args = base_args();
        args.prompt = Some("work".to_string());
        let err = run_with_sender(args, cfg.clone(), |_, _, _| Ok(())).unwrap_err();
        assert!(err.contains("codex failed"));

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
printf 'done\n' > "$out"
"#,
        );
        let mut args = base_args();
        args.prompt = Some("work".to_string());
        let err =
            run_with_sender(args, cfg, |_, _, _| Err("telegram failed".to_string())).unwrap_err();
        assert_eq!(err, "telegram failed");
    }

    fn test_config(root: &Path) -> Config {
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            xdg_config_home: root.join("config"),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: root.join("config/gateway/config.json"),
            codex_bin: root.join("codex"),
            codex_workdir: root.to_path_buf(),
            codex_model: "gpt-test".to_string(),
            fastfetch_bin: PathBuf::from("fastfetch"),
            state_dir: root.join("state/gateway"),
            chat_state_dir: root.join("state/gateway/chats"),
            cron_state_dir: root.join("state/gateway/cron"),
            offset_file: root.join("state/gateway/telegram.offset"),
            gateway_log_file: root.join("state/gateway/logs/gateway.log"),
            launchd_target: "gui/0/ai.gateway-test".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: std::time::Duration::from_secs(5),
        }
    }

    fn executable(path: PathBuf, body: &str) -> PathBuf {
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
