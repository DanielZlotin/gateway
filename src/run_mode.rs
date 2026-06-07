use crate::cli::RunArgs;
use crate::codex::{run_codex, CodexConfig};
use crate::config::Config;
use crate::telegram::TelegramClient;
use crate::text::{is_ok_response, redact_private_data, split_telegram_message};
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

fn target_chat_id(args: &RunArgs, cfg: &Config) -> Result<i64, String> {
    let Some(chat_id) = args.chat else {
        return cfg
            .telegram_chat_ids
            .first()
            .copied()
            .ok_or_else(|| "GATEWAY_TELEGRAM_CHAT_ID must include at least one id".to_string());
    };
    if cfg.bot_token_for_chat(chat_id).is_some() {
        Ok(chat_id)
    } else {
        Err(format!("chat {chat_id} is not in GATEWAY_TELEGRAM_CHAT_ID"))
    }
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
    send_telegram: impl FnMut(&str, i64, &str) -> Result<(), String>,
) -> Result<String, String> {
    let codex = CodexConfig::from(&cfg);
    run_with_sender_and_codex(args, cfg, &codex, send_telegram)
}

fn run_with_sender_and_codex(
    args: RunArgs,
    cfg: Config,
    codex: &CodexConfig,
    mut send_telegram: impl FnMut(&str, i64, &str) -> Result<(), String>,
) -> Result<String, String> {
    let prompt = load_prompt(&args, std::io::stdin())?;
    let default_provider_model = cfg.default_provider_model();
    let model = args
        .model
        .as_deref()
        .unwrap_or(&default_provider_model.model);
    let chat_id = target_chat_id(&args, &cfg)?;
    let output = run_codex(
        codex,
        &prompt,
        None,
        default_provider_model.provider,
        model,
        cfg.codex_timeout,
        &cfg.state_dir,
    )?;
    if should_send_telegram_result(&output.final_text) {
        let telegram_text = redact_private_data(&output.final_text);
        for part in split_telegram_message(&telegram_text) {
            let bot_token = cfg
                .bot_token_for_chat(chat_id)
                .ok_or_else(|| format!("chat {chat_id} is not in GATEWAY_TELEGRAM_CHAT_ID"))?;
            send_telegram(bot_token, chat_id, &part)?;
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
            prompt: None,
            prompt_file: None,
            model: None,
            chat: None,
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
printf 'session id: session-cli\n' >&2
"#,
            ),
        );
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let mut args = base_args();
        args.prompt = Some("finish quietly".to_string());

        let output =
            run_with_sender_and_codex(args, cfg.clone(), &codex, move |token, chat_id, text| {
                sent.lock()
                    .unwrap()
                    .push((token.to_string(), chat_id, text.to_string()));
                Ok(())
            })
            .unwrap();

        assert_eq!(output, "OK");
        assert!(sends.lock().unwrap().is_empty());
        assert!(!cfg.chat_state_dir.exists());
    }

    #[test]
    fn run_mode_sends_non_ok_results_without_saving_session_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.telegram_chat_ids = vec![42, 77];
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
printf 'done\n' > "$out"
printf 'session id: session-run\n' >&2
"#,
            ),
        );
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let args = RunArgs {
            prompt: Some("work".to_string()),
            prompt_file: None,
            model: Some("gpt-override".to_string()),
            chat: None,
        };

        let output =
            run_with_sender_and_codex(args, cfg.clone(), &codex, move |token, chat_id, text| {
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
        assert!(!cfg.chat_state_dir.exists());
    }

    #[test]
    fn run_mode_sends_to_requested_allowed_chat_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.telegram_chat_ids = vec![42, 77];
        cfg.telegram_bots = vec![
            crate::config::TelegramBotConfig {
                bot_token: "token-a".to_string(),
                chat_ids: vec![42],
                offset_file: dir.path().join("state/gateway/telegram-1.offset"),
            },
            crate::config::TelegramBotConfig {
                bot_token: "token-b".to_string(),
                chat_ids: vec![77],
                offset_file: dir.path().join("state/gateway/telegram-2.offset"),
            },
        ];
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
printf 'done\n' > "$out"
"#,
            ),
        );
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let args = RunArgs {
            prompt: Some("work".to_string()),
            prompt_file: None,
            model: None,
            chat: Some(77),
        };

        let output = run_with_sender_and_codex(args, cfg, &codex, move |token, chat_id, text| {
            sent.lock()
                .unwrap()
                .push((token.to_string(), chat_id, text.to_string()));
            Ok(())
        })
        .unwrap();

        assert_eq!(output, "done");
        assert_eq!(
            *sends.lock().unwrap(),
            vec![("token-b".to_string(), 77, "done".to_string())]
        );
    }

    #[test]
    fn run_mode_rejects_requested_chat_outside_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.telegram_chat_ids = vec![42];
        let mut args = base_args();
        args.chat = Some(77);

        let err = target_chat_id(&args, &cfg).unwrap_err();

        assert!(err.contains("not in GATEWAY_TELEGRAM_CHAT_ID"));
    }

    #[test]
    fn run_mode_rejects_unconfigured_chat_before_starting_codex() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.telegram_chat_ids = vec![42];
        let mut args = base_args();
        args.prompt = Some("work".to_string());
        args.chat = Some(77);

        let err = run_with_sender(args, cfg, |_, _, _| Ok(())).unwrap_err();

        assert!(err.contains("not in GATEWAY_TELEGRAM_CHAT_ID"));
        assert!(!err.contains("start codex"));
    }

    #[test]
    fn run_mode_redacts_telegram_result_without_changing_stdout() {
        let dir = tempfile::tempdir().unwrap();
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
        let sends = Arc::new(Mutex::new(Vec::new()));
        let sent = sends.clone();
        let mut args = base_args();
        args.prompt = Some("work".to_string());

        let output = run_with_sender_and_codex(args, cfg, &codex, move |_, _, text| {
            sent.lock().unwrap().push(text.to_string());
            Ok(())
        })
        .unwrap();

        assert!(output.contains("sk-test-secret-value"));
        let sent = sends.lock().unwrap().join("\n");
        assert!(!sent.contains("sk-test-secret-value"));
        assert!(sent.contains("OPENAI_API_KEY=<redacted>"));
    }

    #[test]
    fn run_mode_propagates_codex_and_telegram_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let failing_codex = test_codex_config(
            &cfg,
            executable(
                dir.path().join("codex-fails"),
                r#"#!/bin/sh
printf 'codex failed\n' >&2
exit 2
"#,
            ),
        );
        let mut args = base_args();
        args.prompt = Some("work".to_string());
        let err = run_with_sender_and_codex(args, cfg.clone(), &failing_codex, |_, _, _| Ok(()))
            .unwrap_err();
        assert!(err.contains("codex failed"));

        let final_codex = test_codex_config(
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
printf 'done\n' > "$out"
"#,
            ),
        );
        let mut args = base_args();
        args.prompt = Some("work".to_string());
        let err = run_with_sender_and_codex(args, cfg, &final_codex, |_, _, _| {
            Err("telegram failed".to_string())
        })
        .unwrap_err();
        assert_eq!(err, "telegram failed");
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
            models: vec![crate::config::ProviderModel {
                provider: crate::provider::Provider::Codex,
                model: "gpt-test".to_string(),
                role: crate::config::ModelRole::Default,
            }],
            tts: None,
            state_dir: root.join("state/gateway"),
            chat_state_dir: root.join("state/gateway/chats"),
            offset_file: root.join("state/gateway/telegram.offset"),
            gateway_log_file: root.join("state/gateway/logs/gateway.log"),
            launchd_target: "gui/0/ai.gateway-test".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: std::time::Duration::from_secs(5),
        }
    }

    fn test_codex_config(cfg: &Config, bin: PathBuf) -> CodexConfig {
        CodexConfig {
            bin,
            workdir: cfg.codex_workdir.clone(),
            default_model: cfg.default_provider_model().model.clone(),
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
