use crate::cli::RunArgs;
use crate::config::Config;
use crate::logs;
use crate::run_mode;
use crate::update::{run_gateway_update_inline, GatewayUpdateRun};
use chrono::{Local, NaiveDate, Timelike};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const HEARTBEAT_ACTIVE_ENV: &str = "GATEWAY_HEARTBEAT_ACTIVE";

#[derive(Debug, PartialEq, Eq)]
enum HeartbeatDecision {
    Run(i64),
    MarkOnly(i64),
    Skip,
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
    append_heartbeat_log(&cfg, "INFO", "starting gateway heartbeat")?;

    let interval_minutes = cfg.heartbeat_interval.as_secs() / 60;
    let boundary = heartbeat_boundary_minutes(now_minutes, interval_minutes);
    let state_file = heartbeat_state_file(&cfg);
    let last_boundary = read_heartbeat_boundary(&state_file)?;
    let at_boundary = now_minutes == boundary;

    match heartbeat_decision(last_boundary, boundary, at_boundary) {
        HeartbeatDecision::Skip => {
            append_heartbeat_log(&cfg, "INFO", "heartbeat not due")?;
            return Ok("heartbeat not due".to_string());
        }
        HeartbeatDecision::MarkOnly(boundary) => {
            write_heartbeat_boundary(&state_file, boundary)?;
            append_heartbeat_log(&cfg, "INFO", "heartbeat initialized")?;
            return Ok("heartbeat initialized".to_string());
        }
        HeartbeatDecision::Run(boundary) => {
            write_heartbeat_boundary(&state_file, boundary)?;
        }
    }

    match run_update(&cfg) {
        Ok(GatewayUpdateRun::Completed) => {
            append_heartbeat_log(&cfg, "INFO", "heartbeat update completed")?
        }
        Ok(GatewayUpdateRun::AlreadyRunning) => {
            append_heartbeat_log(&cfg, "WARN", "gateway update already running")?;
            return Ok("gateway update already running".to_string());
        }
        Err(err) => {
            append_heartbeat_log(&cfg, "ERROR", &format!("heartbeat update failed: {err}"))?;
            return Err(err);
        }
    }

    let heartbeat_file = cfg.xdg_config_home.join("gateway/HEARTBEAT.md");
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
        Ok(_) => append_heartbeat_log(&cfg, "INFO", "heartbeat completed")?,
        Err(err) => append_heartbeat_log(&cfg, "ERROR", &format!("heartbeat failed: {err}"))?,
    }
    result
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

fn heartbeat_log_file(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("logs/heartbeat.log")
}

fn append_heartbeat_log(cfg: &Config, level: &str, message: &str) -> Result<(), String> {
    append_log_line(&heartbeat_log_file(cfg), level, message)
}

fn append_log_line(path: &Path, level: &str, message: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create heartbeat log dir: {err}"))?;
    }
    let line = logs::format_log_line(
        &logs::current_utc_timestamp(),
        env!("CARGO_PKG_VERSION"),
        level,
        message,
    );
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| format!("open heartbeat log: {err}"))?;
    writeln!(file, "{line}").map_err(|err| format!("write heartbeat log: {err}"))
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
    fn heartbeat_writes_log_under_derived_state_dir() {
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
        let log = fs::read_to_string(cfg.state_dir.join("logs/heartbeat.log")).unwrap();
        assert!(log.contains("starting gateway heartbeat"));
        assert!(log.contains("heartbeat completed"));
    }

    fn test_config(root: &Path) -> Config {
        let xdg_config_home = root.join("config");
        let state_dir = root.join("state/gateway");
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
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
