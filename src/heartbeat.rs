use crate::cli::RunArgs;
use crate::config::Config;
use crate::logs;
use crate::run_mode;
use crate::update::{run_gateway_update_inline, GatewayUpdateRun};
use chrono::{Local, NaiveDate, Timelike};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const HEARTBEAT_ACTIVE_ENV: &str = "GATEWAY_HEARTBEAT_ACTIVE";

#[derive(Debug, PartialEq, Eq)]
enum HeartbeatDecision {
    Run(i64),
    MarkOnly(i64),
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeartbeatRunState {
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub result: String,
    pub message: String,
}

pub fn run(cfg: Config) -> Result<String, String> {
    std::env::set_var(HEARTBEAT_ACTIVE_ENV, "1");
    run_due_heartbeat(
        cfg,
        current_total_minutes(),
        run_gateway_update_inline,
        run_mode::run,
    )
}

fn run_due_heartbeat(
    cfg: Config,
    now_minutes: i64,
    run_update: impl FnOnce(&Config) -> Result<GatewayUpdateRun, String>,
    run_prompt: impl FnOnce(RunArgs, Config) -> Result<String, String>,
) -> Result<String, String> {
    fs::create_dir_all(&cfg.state_dir)
        .map_err(|err| format!("create heartbeat state dir: {err}"))?;

    let interval_minutes = cfg.heartbeat_interval.as_secs() / 60;
    let boundary = heartbeat_boundary_minutes(now_minutes, interval_minutes);
    let state_file = heartbeat_state_file(&cfg);
    let run_state_missing = !heartbeat_run_state_file(&cfg)
        .try_exists()
        .map_err(|err| format!("read heartbeat state: {err}"))?;
    let last_boundary = read_heartbeat_boundary(&state_file)?;
    let at_boundary = now_minutes == boundary;

    match heartbeat_decision(last_boundary, boundary, at_boundary) {
        HeartbeatDecision::Skip => {
            if run_state_missing {
                write_heartbeat_run_state(
                    &cfg,
                    HeartbeatRunState::finished("initialized", "initialized"),
                )?;
                append_heartbeat_log(&cfg, "INFO", "🫀 hb init")?;
            }
            return Ok("heartbeat not due".to_string());
        }
        HeartbeatDecision::MarkOnly(boundary) => {
            write_heartbeat_boundary(&state_file, boundary)?;
            write_heartbeat_run_state(
                &cfg,
                HeartbeatRunState::finished("initialized", "initialized"),
            )?;
            append_heartbeat_log(&cfg, "INFO", "🫀 hb init")?;
            return Ok("heartbeat initialized".to_string());
        }
        HeartbeatDecision::Run(boundary) => {
            write_heartbeat_boundary(&state_file, boundary)?;
        }
    }

    let started_at = logs::current_utc_timestamp();
    write_heartbeat_run_state(&cfg, HeartbeatRunState::running(&started_at))?;
    append_heartbeat_log(&cfg, "INFO", "🫀 hb start")?;

    match run_update(&cfg) {
        Ok(GatewayUpdateRun::Completed) => append_heartbeat_log(&cfg, "INFO", "🫀 hb update ok")?,
        Ok(GatewayUpdateRun::AlreadyRunning) => {
            write_heartbeat_run_state(
                &cfg,
                HeartbeatRunState::finished_from(&started_at, "update-running", "update busy"),
            )?;
            append_heartbeat_log(&cfg, "WARN", "🫀 hb update busy")?;
            return Ok("gateway update already running".to_string());
        }
        Err(err) => {
            write_heartbeat_run_state(
                &cfg,
                HeartbeatRunState::finished_from(&started_at, "failed", &err),
            )?;
            append_heartbeat_log(&cfg, "ERROR", &format!("🫀 hb update failed: {err}"))?;
            return Err(err);
        }
    }

    let heartbeat_file = crate::context::ensure_heartbeat_prompt_file(&cfg.xdg_config_home)?;
    let result = run_prompt(
        RunArgs {
            prompt: None,
            prompt_file: Some(heartbeat_file),
            model: None,
            chat: None,
        },
        cfg.clone(),
    );
    match &result {
        Ok(output) => {
            write_heartbeat_run_state(
                &cfg,
                HeartbeatRunState::finished_from(&started_at, "completed", output),
            )?;
            append_heartbeat_log(&cfg, "INFO", "🫀 hb done")?;
        }
        Err(err) => {
            write_heartbeat_run_state(
                &cfg,
                HeartbeatRunState::finished_from(&started_at, "failed", err),
            )?;
            append_heartbeat_log(&cfg, "ERROR", &format!("🫀 hb failed: {err}"))?;
        }
    }
    result
}

impl HeartbeatRunState {
    fn running(started_at: &str) -> Self {
        Self {
            started_at: started_at.to_string(),
            finished_at: None,
            result: "running".to_string(),
            message: "started".to_string(),
        }
    }

    fn finished(result: &str, message: &str) -> Self {
        let now = logs::current_utc_timestamp();
        Self::finished_from(&now, result, message)
    }

    fn finished_from(started_at: &str, result: &str, message: &str) -> Self {
        Self {
            started_at: started_at.to_string(),
            finished_at: Some(logs::current_utc_timestamp()),
            result: result.to_string(),
            message: message.to_string(),
        }
    }
}

fn current_total_minutes() -> i64 {
    let now = Local::now();
    let anchor = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid heartbeat anchor date");
    let days = now.date_naive().signed_duration_since(anchor).num_days();
    days * 24 * 60 + i64::from(now.hour()) * 60 + i64::from(now.minute())
}

fn heartbeat_boundary_minutes(total_minutes: i64, interval_minutes: u64) -> i64 {
    let interval = i64::try_from(interval_minutes).expect("heartbeat interval fits in i64 minutes");
    total_minutes.div_euclid(interval) * interval
}

fn heartbeat_decision(
    last_boundary: Option<i64>,
    current_boundary: i64,
    at_boundary: bool,
) -> HeartbeatDecision {
    match last_boundary {
        None if at_boundary => HeartbeatDecision::Run(current_boundary),
        None => HeartbeatDecision::MarkOnly(current_boundary),
        Some(last) if current_boundary > last => HeartbeatDecision::Run(current_boundary),
        Some(_) => HeartbeatDecision::Skip,
    }
}

fn heartbeat_state_file(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("heartbeat.last")
}

fn heartbeat_run_state_file(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("heartbeat.json")
}

fn append_heartbeat_log(cfg: &Config, level: &str, message: &str) -> Result<(), String> {
    logs::append_log_file(&cfg.gateway_log_file, level, message)
}

pub fn read_heartbeat_run_state(cfg: &Config) -> Result<Option<HeartbeatRunState>, String> {
    let path = heartbeat_run_state_file(cfg);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("read heartbeat state: {err}")),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|err| format!("parse heartbeat state: {err}"))
}

fn write_heartbeat_run_state(cfg: &Config, state: HeartbeatRunState) -> Result<(), String> {
    let path = heartbeat_run_state_file(cfg);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create heartbeat state dir: {err}"))?;
    }
    let text = serde_json::to_string_pretty(&state)
        .map_err(|err| format!("serialize heartbeat state: {err}"))?;
    fs::write(path, format!("{text}\n")).map_err(|err| format!("write heartbeat state: {err}"))
}

fn read_heartbeat_boundary(path: &Path) -> Result<Option<i64>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("read heartbeat state: {err}")),
    };
    text.trim()
        .parse::<i64>()
        .map(Some)
        .map_err(|err| format!("parse heartbeat state: {err}"))
}

fn write_heartbeat_boundary(path: &Path, boundary: i64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create heartbeat state dir: {err}"))?;
    }
    fs::write(path, format!("{boundary}\n")).map_err(|err| format!("write heartbeat state: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ModelRole, ProviderModel, TelegramBotConfig};
    use crate::provider::Provider;
    use std::cell::Cell;
    use std::time::Duration;
    use tempfile::tempdir;

    const HEARTBEAT_PROMPT_HEADER: &str =
        "# HEARTBEAT.md\n> **Scope:** scheduled heartbeat protocol only.\n";
    const HEARTBEAT_PROMPT_TEMPLATE: &str = "# HEARTBEAT.md\n> **Scope:** scheduled heartbeat protocol only.\n\nCheck that the gateway is healthy after its scheduled update.\n\nReturn exactly `OK` if no action is needed. Otherwise, return one concise status\nmessage describing the issue.\n";

    #[test]
    fn heartbeat_schedule_is_anchored_to_midnight() {
        let one_day = 24 * 60;

        assert_eq!(heartbeat_boundary_minutes(one_day + 179, 180), one_day);
        assert_eq!(
            heartbeat_boundary_minutes(one_day + 180, 180),
            one_day + 180
        );
        assert_eq!(
            heartbeat_boundary_minutes(one_day + 359, 180),
            one_day + 180
        );
        assert_eq!(
            heartbeat_boundary_minutes(one_day + 360, 180),
            one_day + 360
        );
    }

    #[test]
    fn heartbeat_decision_runs_first_execution_only_on_boundary() {
        assert_eq!(
            heartbeat_decision(None, 24 * 60, false),
            HeartbeatDecision::MarkOnly(24 * 60)
        );
        assert_eq!(
            heartbeat_decision(None, 24 * 60, true),
            HeartbeatDecision::Run(24 * 60)
        );
    }

    #[test]
    fn heartbeat_decision_runs_when_new_boundary_is_due() {
        assert_eq!(
            heartbeat_decision(Some(24 * 60), 24 * 60 + 180, false),
            HeartbeatDecision::Run(24 * 60 + 180)
        );
        assert_eq!(
            heartbeat_decision(Some(24 * 60 + 180), 24 * 60 + 180, true),
            HeartbeatDecision::Skip
        );
    }

    #[test]
    fn heartbeat_does_not_use_a_process_lock() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::create_dir_all(cfg.xdg_config_home.join("gateway")).unwrap();
        fs::write(
            cfg.xdg_config_home.join("gateway/HEARTBEAT.md"),
            "# Old heartbeat\n> Old scope\n\ncustom heartbeat prompt\n",
        )
        .unwrap();
        fs::write(cfg.state_dir.join("heartbeat.last"), "0\n").unwrap();
        fs::write(
            cfg.state_dir.join("heartbeat.lock"),
            format!("{}\n", std::process::id()),
        )
        .unwrap();
        let update_ran = Cell::new(false);
        let prompt_ran = Cell::new(false);

        let output = run_due_heartbeat(
            cfg.clone(),
            60,
            |_| {
                update_ran.set(true);
                Ok(GatewayUpdateRun::Completed)
            },
            |args, cfg| {
                prompt_ran.set(true);
                assert_eq!(
                    args.prompt_file,
                    Some(cfg.xdg_config_home.join("gateway/HEARTBEAT.md"))
                );
                assert_eq!(
                    fs::read_to_string(cfg.xdg_config_home.join("gateway/HEARTBEAT.md")).unwrap(),
                    format!("{HEARTBEAT_PROMPT_HEADER}\ncustom heartbeat prompt\n")
                );
                Ok("heartbeat body ran".to_string())
            },
        )
        .unwrap();

        assert_eq!(output, "heartbeat body ran");
        assert!(update_ran.get());
        assert!(prompt_ran.get());
        assert!(cfg.state_dir.join("heartbeat.lock").exists());
    }

    #[test]
    fn heartbeat_creates_custom_prompt_file_from_context_template_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::write(cfg.state_dir.join("heartbeat.last"), "0\n").unwrap();
        let heartbeat_file = cfg.xdg_config_home.join("gateway/HEARTBEAT.md");

        let output = run_due_heartbeat(
            cfg.clone(),
            60,
            |_| Ok(GatewayUpdateRun::Completed),
            |args, cfg| {
                assert_eq!(
                    args.prompt_file,
                    Some(cfg.xdg_config_home.join("gateway/HEARTBEAT.md"))
                );
                assert!(heartbeat_file.exists());
                let prompt = fs::read_to_string(&heartbeat_file).unwrap();
                assert_eq!(prompt, HEARTBEAT_PROMPT_TEMPLATE);
                Ok("heartbeat body ran".to_string())
            },
        )
        .unwrap();

        assert_eq!(output, "heartbeat body ran");
    }

    #[test]
    fn heartbeat_initializes_missing_run_state_when_not_due() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::write(cfg.state_dir.join("heartbeat.last"), "60\n").unwrap();

        let output = run_due_heartbeat(
            cfg.clone(),
            60,
            |_| panic!("heartbeat update should not run"),
            |_, _| panic!("heartbeat prompt should not run"),
        )
        .unwrap();

        assert_eq!(output, "heartbeat not due");
        let state = fs::read_to_string(cfg.state_dir.join("heartbeat.json")).unwrap();
        assert!(state.contains(r#""result": "initialized""#), "{state}");
        assert!(state.contains(r#""message": "initialized""#), "{state}");
    }

    #[test]
    fn heartbeat_writes_canonical_gateway_log_and_state() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::write(cfg.state_dir.join("heartbeat.last"), "0\n").unwrap();

        let output = run_due_heartbeat(
            cfg.clone(),
            60,
            |_| Ok(GatewayUpdateRun::Completed),
            |_, _| Ok("heartbeat body ran".to_string()),
        )
        .unwrap();

        assert_eq!(output, "heartbeat body ran");
        let log = fs::read_to_string(&cfg.gateway_log_file).unwrap();
        assert!(log.contains("🫀 hb start"));
        assert!(log.contains("🫀 hb done"));
        assert!(!cfg.state_dir.join("logs/heartbeat.log").exists());

        let state = fs::read_to_string(cfg.state_dir.join("heartbeat.json")).unwrap();
        assert!(state.contains(r#""result": "completed""#), "{state}");
        assert!(
            state.contains(r#""message": "heartbeat body ran""#),
            "{state}"
        );
    }

    #[test]
    fn heartbeat_state_records_failure() {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        fs::create_dir_all(&cfg.state_dir).unwrap();
        fs::write(cfg.state_dir.join("heartbeat.last"), "0\n").unwrap();

        let err = run_due_heartbeat(
            cfg.clone(),
            60,
            |_| Ok(GatewayUpdateRun::Completed),
            |_, _| Err("prompt failed".to_string()),
        )
        .unwrap_err();

        assert_eq!(err, "prompt failed");
        let state = fs::read_to_string(cfg.state_dir.join("heartbeat.json")).unwrap();
        assert!(state.contains(r#""result": "failed""#), "{state}");
        assert!(state.contains(r#""message": "prompt failed""#), "{state}");
    }

    fn test_config(root: &Path) -> Config {
        let xdg_config_home = root.join("config");
        let state_dir = root.join("state/gateway");
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            default_telegram_chat_id: 42,
            telegram_bots: vec![TelegramBotConfig {
                bot_token: "token".to_string(),
                chat_ids: vec![42],
                offset_file: state_dir.join("telegram.offset"),
            }],
            xdg_config_home: xdg_config_home.clone(),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: xdg_config_home.join("gateway/config.json"),
            codex_workdir: root.to_path_buf(),
            models: vec![ProviderModel {
                provider: Provider::Codex,
                model: "gpt-test".to_string(),
                role: ModelRole::Default,
            }],
            tts: None,
            state_dir: state_dir.clone(),
            chat_state_dir: state_dir.join("chats"),
            offset_file: state_dir.join("telegram.offset"),
            gateway_log_file: state_dir.join("logs/gateway.log"),
            launchd_target: "gui/<uid>/ai.gateway".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(60),
        }
    }
}
