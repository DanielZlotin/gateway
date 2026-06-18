use crate::cli::ChatArgs;
use crate::codex::CodexConfig;
use crate::config::Config;
use crate::session::{SessionKey, SessionStore};
use crate::status::{format_status_message, status_sections, StatusSections};
use crate::update::{run_gateway_update_inline, GatewayUpdateRun};

pub fn list(args: ChatArgs, cfg: Config) -> Result<String, String> {
    let (store, key) = session_context(&cfg, &args)?;
    Ok(store.list(&key))
}

pub fn status(args: ChatArgs, cfg: Config) -> Result<String, String> {
    let codex = CodexConfig::from(&cfg);
    let sections = status_sections(&cfg, &codex);
    status_with_sections(args, cfg, sections)
}

pub fn update(cfg: Config) -> Result<String, String> {
    update_with_runner(cfg, run_gateway_update_inline)
}

fn status_with_sections(
    args: ChatArgs,
    cfg: Config,
    sections: StatusSections,
) -> Result<String, String> {
    let (store, key) = session_context(&cfg, &args)?;
    let state = store.load(&key);
    Ok(format_status_message(
        &state,
        &sections.heartbeat,
        &sections.codex,
        &sections.git,
        &sections.fetch,
    ))
}

fn update_with_runner(
    cfg: Config,
    run_update: impl FnOnce(&Config) -> Result<GatewayUpdateRun, String>,
) -> Result<String, String> {
    match run_update(&cfg)? {
        GatewayUpdateRun::Completed => Ok("gateway update completed".to_string()),
        GatewayUpdateRun::AlreadyRunning => Ok("gateway update already running".to_string()),
    }
}

fn session_context(cfg: &Config, args: &ChatArgs) -> Result<(SessionStore, SessionKey), String> {
    let chat_id = cfg.resolve_chat_id(args.chat)?;
    let default_model = cfg.default_provider_model();
    let store = SessionStore::new_with_provider(
        cfg.chat_state_dir.clone(),
        default_model.model.clone(),
        default_model.provider,
    );
    Ok((store, session_key(chat_id)))
}

fn session_key(chat_id: i64) -> SessionKey {
    SessionKey::Chat {
        chat_id,
        thread_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ChatArgs;
    use crate::config::{Config, ModelRole, ProviderModel, TelegramBotConfig};
    use crate::provider::Provider;
    use crate::session::SessionStore;
    use crate::status::StatusSections;
    use crate::update::GatewayUpdateRun;
    use std::time::Duration;

    #[test]
    fn list_prints_sessions_for_default_chat() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());
        let store = test_store(&cfg);
        let key = session_key(42);
        let state = store.load(&key);
        store
            .save_current_session(&key, state.generation, "session-a")
            .unwrap();
        store.rename_current(&key, "Main work").unwrap();

        let output = list(ChatArgs { chat: None }, cfg).unwrap();

        assert!(output.contains("💾 Saved sessions:"));
        assert!(output.contains("⭐"));
        assert!(output.contains("Main work"));
    }

    #[test]
    fn list_rejects_unconfigured_chat() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());

        let err = list(ChatArgs { chat: Some(99) }, cfg).unwrap_err();

        assert_eq!(err, "chat 99 is not in GATEWAY_TELEGRAM_CHAT_ID");
    }

    #[test]
    fn status_prints_sections_for_requested_chat() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.telegram_chat_ids = vec![42, 77];
        cfg.telegram_bots[0].chat_ids = vec![42, 77];
        let store = test_store(&cfg);
        let key = session_key(77);
        let state = store.load(&key);
        store
            .save_current_session(&key, state.generation, "session-status")
            .unwrap();

        let output = status_with_sections(
            ChatArgs { chat: Some(77) },
            cfg,
            StatusSections {
                heartbeat: "🫀 Heartbeat: done 12:00".to_string(),
                codex: "🧠 Codex: ok".to_string(),
                git: "🧾 Git: clean".to_string(),
                fetch: "🖥️ fastfetch: ok".to_string(),
            },
        )
        .unwrap();

        assert!(output.contains("📦 Gateway version:"));
        assert!(output.contains("🤖 Model: gpt-default"));
        assert!(output.contains("🫀 Heartbeat: done 12:00"));
        assert!(output.contains("🧠 Codex: ok"));
        assert!(output.contains("🧾 Git: clean"));
        assert!(output.contains("🖥️ fastfetch: ok"));
    }

    #[test]
    fn update_reports_completed_or_already_running() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());

        assert_eq!(
            update_with_runner(cfg.clone(), |_| Ok(GatewayUpdateRun::Completed)).unwrap(),
            "gateway update completed"
        );
        assert_eq!(
            update_with_runner(cfg, |_| Ok(GatewayUpdateRun::AlreadyRunning)).unwrap(),
            "gateway update already running"
        );
    }

    fn test_config(root: &std::path::Path) -> Config {
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            default_telegram_chat_id: 42,
            telegram_bots: vec![TelegramBotConfig {
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
            models: vec![ProviderModel {
                provider: Provider::Codex,
                model: "gpt-default".to_string(),
                role: ModelRole::Default,
            }],
            tts: None,
            state_dir: root.join("state/gateway"),
            chat_state_dir: root.join("state/gateway/chats"),
            offset_file: root.join("state/gateway/telegram.offset"),
            gateway_log_file: root.join("state/gateway/logs/gateway.log"),
            launchd_target: "gui/0/ai.gateway-test".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(24 * 60 * 60),
        }
    }

    fn test_store(cfg: &Config) -> SessionStore {
        let default_model = cfg.default_provider_model();
        SessionStore::new_with_provider(
            cfg.chat_state_dir.clone(),
            default_model.model.clone(),
            default_model.provider,
        )
    }
}
