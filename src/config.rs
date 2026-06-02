use crate::json_file::{save_pretty_json, SaveJsonLabels};
use crate::launchd;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const GATEWAY_TELEGRAM_TOKEN_ENV: &str = "GATEWAY_TELEGRAM_TOKEN";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-4-8";
pub const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-5.5";
pub const DEFAULT_CODEX_TIMEOUT_MINS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bot_token: String,
    pub telegram_chat_ids: Vec<i64>,
    pub xdg_config_home: PathBuf,
    pub xdg_cache_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub gateway_config_file: PathBuf,
    pub codex_bin: PathBuf,
    pub codex_workdir: PathBuf,
    pub models: Vec<ProviderModel>,
    pub fastfetch_bin: PathBuf,
    pub state_dir: PathBuf,
    pub chat_state_dir: PathBuf,
    pub offset_file: PathBuf,
    pub gateway_log_file: PathBuf,
    pub launchd_target: String,
    pub poll_timeout_sec: u64,
    pub queue_depth: usize,
    pub codex_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfigFile {
    pub models: Vec<ProviderModel>,
    #[serde(default = "default_timeout_mins")]
    pub timeout_mins: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    pub provider: Provider,
    pub model: String,
}

pub fn current_env() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

pub fn load() -> Result<Config, String> {
    load_from_env(&current_env())
}

pub fn load_from_env(env: &BTreeMap<String, String>) -> Result<Config, String> {
    let bot_token = required(env, GATEWAY_TELEGRAM_TOKEN_ENV)?;
    let xdg_config_home = resolve_xdg_config_home(env)?;
    let xdg_cache_home = resolve_xdg_cache_home(env)?;
    let xdg_data_home = resolve_xdg_data_home(env)?;
    let xdg_state_home = resolve_xdg_state_home(env)?;
    let gateway_config_file = xdg_config_home.join("gateway/config.json");
    let gateway_config = load_gateway_config(&gateway_config_file)?;
    let state_dir = xdg_state_home.join("gateway");
    let chat_state_dir = state_dir.join("chats");
    let launchd_target = launchd::target()?;

    Ok(Config {
        bot_token,
        telegram_chat_ids: telegram_chat_ids(env)?,
        xdg_config_home: xdg_config_home.clone(),
        xdg_cache_home,
        xdg_data_home,
        xdg_state_home,
        gateway_config_file,
        codex_bin: PathBuf::from("codex"),
        codex_workdir: path(env, "GATEWAY_CODEX_WORKDIR", xdg_config_home),
        models: gateway_config.models,
        fastfetch_bin: PathBuf::from("fastfetch"),
        state_dir: state_dir.clone(),
        chat_state_dir,
        offset_file: state_dir.join("telegram.offset"),
        gateway_log_file: state_dir.join("logs/gateway.log"),
        launchd_target,
        poll_timeout_sec: 50,
        queue_depth: 8,
        codex_timeout: Duration::from_secs(timeout_secs(gateway_config.timeout_mins)?),
    })
}

pub fn load_gateway_config(path: &Path) -> Result<GatewayConfigFile, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let cfg = GatewayConfigFile::default();
            save_gateway_config(path, &cfg)?;
            return Ok(cfg);
        }
        Err(err) => return Err(format!("read gateway config: {err}")),
    };
    let mut cfg: GatewayConfigFile =
        serde_json::from_str(&text).map_err(|err| format!("parse gateway config: {err}"))?;
    cfg.normalize()?;
    save_gateway_config(path, &cfg)?;
    Ok(cfg)
}

pub fn save_gateway_config(path: &Path, cfg: &GatewayConfigFile) -> Result<(), String> {
    save_pretty_json(
        path,
        cfg,
        SaveJsonLabels {
            create_dir: "create gateway config dir",
            write: "write gateway config",
            replace: "replace gateway config",
        },
    )
}

fn required(env: &BTreeMap<String, String>, key: &str) -> Result<String, String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{key} is required"))
}

fn path(env: &BTreeMap<String, String>, key: &str, default: PathBuf) -> PathBuf {
    optional_path(env, key).unwrap_or(default)
}

pub fn resolve_xdg_config_home(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    xdg_path(env, "XDG_CONFIG_HOME", ".config")
}

pub fn resolve_xdg_cache_home(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    xdg_path(env, "XDG_CACHE_HOME", ".cache")
}

pub fn resolve_xdg_data_home(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    xdg_path(env, "XDG_DATA_HOME", ".local/share")
}

pub fn resolve_xdg_state_home(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    xdg_path(env, "XDG_STATE_HOME", ".local/state")
}

fn xdg_path(
    env: &BTreeMap<String, String>,
    key: &str,
    home_relative_default: &str,
) -> Result<PathBuf, String> {
    match env.get(key) {
        Some(value) if !value.trim().is_empty() => Ok(PathBuf::from(value.trim())),
        _ => Ok(PathBuf::from(required(env, "HOME")?).join(home_relative_default)),
    }
}

fn optional(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn optional_path(env: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    optional(env, key).map(PathBuf::from)
}

fn telegram_chat_ids(env: &BTreeMap<String, String>) -> Result<Vec<i64>, String> {
    let raw = required(env, "GATEWAY_TELEGRAM_CHAT_IDS")?;
    let mut ids = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let id = part.parse::<i64>().map_err(|_| {
            "GATEWAY_TELEGRAM_CHAT_IDS must contain comma-separated integers".to_string()
        })?;
        if id <= 0 {
            return Err("GATEWAY_TELEGRAM_CHAT_IDS must contain private chat ids only".to_string());
        }
        ids.push(id);
    }
    if ids.is_empty() {
        return Err("GATEWAY_TELEGRAM_CHAT_IDS must include at least one id".to_string());
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

impl Default for GatewayConfigFile {
    fn default() -> Self {
        Self {
            models: default_models(),
            timeout_mins: default_timeout_mins(),
        }
    }
}

impl GatewayConfigFile {
    pub fn normalize(&mut self) -> Result<(), String> {
        normalize_models(&mut self.models)?;
        if self.timeout_mins == 0 {
            return Err("timeout_mins must be greater than zero".to_string());
        }
        Ok(())
    }
}

pub fn default_models() -> Vec<ProviderModel> {
    vec![
        ProviderModel {
            provider: Provider::Codex,
            model: DEFAULT_CODEX_MODEL.to_string(),
        },
        ProviderModel {
            provider: Provider::Claude,
            model: DEFAULT_CLAUDE_MODEL.to_string(),
        },
        ProviderModel {
            provider: Provider::Openrouter,
            model: DEFAULT_OPENROUTER_MODEL.to_string(),
        },
    ]
}

const fn default_timeout_mins() -> u64 {
    DEFAULT_CODEX_TIMEOUT_MINS
}

fn timeout_secs(timeout_mins: u64) -> Result<u64, String> {
    timeout_mins
        .checked_mul(60)
        .ok_or_else(|| "timeout_mins is too large".to_string())
}

impl Config {
    pub fn default_provider_model(&self) -> &ProviderModel {
        self.models
            .first()
            .expect("gateway config normalization ensures at least one model")
    }

    pub fn provider_model_at(&self, index: usize) -> Option<&ProviderModel> {
        self.models.get(index)
    }

    pub fn paths_report(&self) -> String {
        let mut lines: Vec<String> = [
            ("xdg_config_home", self.xdg_config_home.as_path()),
            ("xdg_cache_home", self.xdg_cache_home.as_path()),
            ("xdg_data_home", self.xdg_data_home.as_path()),
            ("xdg_state_home", self.xdg_state_home.as_path()),
            ("gateway_config_file", self.gateway_config_file.as_path()),
            ("codex_bin", self.codex_bin.as_path()),
            ("codex_workdir", self.codex_workdir.as_path()),
            ("fastfetch_bin", self.fastfetch_bin.as_path()),
            ("state_dir", self.state_dir.as_path()),
            ("chat_state_dir", self.chat_state_dir.as_path()),
            ("offset_file", self.offset_file.as_path()),
            ("gateway_log_file", self.gateway_log_file.as_path()),
        ]
        .into_iter()
        .map(|(name, path)| format!("{name}={}", path.display()))
        .collect();
        if let Ok(path) = launchd::plist_path() {
            lines.push(format!("launch_agent_plist={}", path.display()));
        }
        lines.join("\n")
    }
}

fn normalize_models(models: &mut Vec<ProviderModel>) -> Result<(), String> {
    for item in models.iter_mut() {
        item.model = item.model.trim().to_string();
    }
    models.retain(|item| !item.model.is_empty());
    if models.is_empty() {
        return Err("gateway config must include at least one non-empty model".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with_token() -> (tempfile::TempDir, BTreeMap<String, String>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cfg_path = root.join("config/gateway/config.json");
        fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        fs::write(
            &cfg_path,
            serde_json::to_string_pretty(&GatewayConfigFile::default()).unwrap(),
        )
        .unwrap();
        let env = BTreeMap::from([
            (GATEWAY_TELEGRAM_TOKEN_ENV.to_string(), "token".to_string()),
            ("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), "42".to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                root.join("config").to_string_lossy().to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_string(),
                root.join("cache").to_string_lossy().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                root.join("data").to_string_lossy().to_string(),
            ),
            (
                "XDG_STATE_HOME".to_string(),
                root.join("state").to_string_lossy().to_string(),
            ),
        ]);
        (dir, env)
    }

    #[test]
    fn loads_required_env_and_gateway_config() {
        let (_dir, env) = env_with_token();
        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.bot_token, "token");
        assert_eq!(cfg.telegram_chat_ids, vec![42]);
        assert!(cfg.xdg_config_home.ends_with("config"));
        assert!(cfg.xdg_cache_home.ends_with("cache"));
        assert!(cfg.xdg_data_home.ends_with("data"));
        assert!(cfg.xdg_state_home.ends_with("state"));
        assert_eq!(cfg.codex_bin, PathBuf::from("codex"));
        assert_eq!(cfg.codex_workdir, cfg.xdg_config_home);
        assert_eq!(cfg.state_dir, cfg.xdg_state_home.join("gateway"));
        assert_eq!(cfg.gateway_log_file, cfg.state_dir.join("logs/gateway.log"));
        assert_eq!(
            cfg.gateway_config_file,
            cfg.xdg_config_home.join("gateway/config.json")
        );
        assert!(cfg.gateway_config_file.exists());
        assert!(cfg.launchd_target.starts_with("gui/"));
        assert!(cfg.launchd_target.ends_with("/ai.gateway"));
        assert_eq!(
            cfg.models,
            vec![
                ProviderModel {
                    provider: Provider::Codex,
                    model: DEFAULT_CODEX_MODEL.to_string()
                },
                ProviderModel {
                    provider: Provider::Claude,
                    model: "claude-opus-4-8".to_string()
                },
                ProviderModel {
                    provider: Provider::Openrouter,
                    model: DEFAULT_OPENROUTER_MODEL.to_string()
                }
            ]
        );
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(30 * 60));

        let text = fs::read_to_string(&cfg.gateway_config_file).unwrap();
        assert!(text.contains(r#""models""#));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("provider").is_none());
        assert!(!text.contains(r#""claude_model""#));
    }

    #[test]
    fn paths_report_lists_runtime_paths() {
        let (_dir, env) = env_with_token();
        let cfg = load_from_env(&env).unwrap();

        let report = cfg.paths_report();

        let labels = report
            .lines()
            .map(|line| line.split_once('=').unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "xdg_config_home",
                "xdg_cache_home",
                "xdg_data_home",
                "xdg_state_home",
                "gateway_config_file",
                "codex_bin",
                "codex_workdir",
                "fastfetch_bin",
                "state_dir",
                "chat_state_dir",
                "offset_file",
                "gateway_log_file",
                "launch_agent_plist",
            ]
        );
    }

    #[test]
    fn blank_xdg_dirs_default_to_home_paths() {
        let (dir, mut env) = env_with_token();
        let home = dir.path().join("home");
        env.insert("HOME".to_string(), home.to_string_lossy().to_string());
        env.insert("XDG_CONFIG_HOME".to_string(), " \t ".to_string());
        env.insert("XDG_CACHE_HOME".to_string(), " \t ".to_string());
        env.insert("XDG_DATA_HOME".to_string(), "".to_string());
        env.insert("XDG_STATE_HOME".to_string(), "".to_string());

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.xdg_config_home, home.join(".config"));
        assert_eq!(cfg.xdg_cache_home, home.join(".cache"));
        assert_eq!(cfg.xdg_data_home, home.join(".local/share"));
        assert_eq!(cfg.xdg_state_home, home.join(".local/state"));
    }

    #[test]
    fn unset_xdg_dirs_default_to_home_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let env = BTreeMap::from([
            (GATEWAY_TELEGRAM_TOKEN_ENV.to_string(), "token".to_string()),
            ("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), "42".to_string()),
            ("HOME".to_string(), home.to_string_lossy().to_string()),
        ]);

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.xdg_config_home, home.join(".config"));
        assert_eq!(cfg.xdg_cache_home, home.join(".cache"));
        assert_eq!(cfg.xdg_data_home, home.join(".local/share"));
        assert_eq!(cfg.xdg_state_home, home.join(".local/state"));
        assert_eq!(
            cfg.gateway_config_file,
            home.join(".config/gateway/config.json")
        );
        assert!(cfg.gateway_config_file.exists());
    }

    #[test]
    fn unset_xdg_dirs_require_home() {
        let env = BTreeMap::from([
            (GATEWAY_TELEGRAM_TOKEN_ENV.to_string(), "token".to_string()),
            ("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), "42".to_string()),
        ]);

        let err = load_from_env(&env).unwrap_err();

        assert_eq!(err, "HOME is required");
    }

    #[test]
    fn loads_gateway_telegram_token() {
        let (_dir, mut env) = env_with_token();
        env.remove(GATEWAY_TELEGRAM_TOKEN_ENV);
        env.insert(
            "GATEWAY_TELEGRAM_TOKEN".to_string(),
            "gateway-token".to_string(),
        );

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.bot_token, "gateway-token");
    }

    #[test]
    fn rejects_missing_bot_token() {
        let err = load_from_env(&BTreeMap::new()).unwrap_err();
        assert!(err.contains(GATEWAY_TELEGRAM_TOKEN_ENV));
    }

    #[test]
    fn rejects_missing_telegram_chat_ids() {
        let (_dir, mut env) = env_with_token();
        env.remove("GATEWAY_TELEGRAM_CHAT_IDS");

        let err = load_from_env(&env).unwrap_err();

        assert!(err.contains("GATEWAY_TELEGRAM_CHAT_IDS"));
    }

    #[test]
    fn parses_supported_overrides() {
        let (_dir, mut env) = env_with_token();
        env.insert("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), "7,8".to_string());
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        fs::write(
            &cfg_path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":9}"#,
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.telegram_chat_ids, vec![7, 8]);
        assert_eq!(
            cfg.default_provider_model(),
            &ProviderModel {
                provider: Provider::Codex,
                model: "gpt-test".to_string()
            }
        );
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(9 * 60));
        assert_eq!(cfg.state_dir, cfg.xdg_state_home.join("gateway"));
        assert_eq!(cfg.gateway_log_file, cfg.state_dir.join("logs/gateway.log"));
        assert_eq!(cfg.poll_timeout_sec, 50);
        assert!(cfg.launchd_target.starts_with("gui/"));
        assert!(cfg.launchd_target.ends_with("/ai.gateway"));
    }

    #[test]
    fn normalizes_gateway_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"openrouter","model":" "},{"provider":"claude","model":"claude-test"}],"timeout_mins":30}"#,
        )
        .unwrap();

        let cfg = load_gateway_config(&path).unwrap();

        assert_eq!(
            cfg.models,
            vec![ProviderModel {
                provider: Provider::Claude,
                model: "claude-test".to_string()
            }]
        );
        let text = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("provider").is_none());
        assert!(text.contains("\"timeout_mins\": 30"));
    }

    #[test]
    fn gateway_config_defaults_missing_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}]}"#,
        )
        .unwrap();

        let cfg = load_gateway_config(&path).unwrap();

        assert_eq!(cfg.timeout_mins, DEFAULT_CODEX_TIMEOUT_MINS);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"timeout_mins\": 30"));
    }

    #[test]
    fn rejects_unknown_config_fields_and_empty_models() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"codex","model":"gpt"}],"timeout_mins":30,"fastfetch":{"args":["--pipe"]}}"#,
        )
        .unwrap();
        let err = load_gateway_config(&path).unwrap_err();
        assert!(err.contains("parse gateway config"));

        fs::write(&path, r#"{"models":[],"timeout_mins":30}"#).unwrap();
        let err = load_gateway_config(&path).unwrap_err();
        assert!(err.contains("at least one"));
    }

    #[test]
    fn rejects_invalid_telegram_chat_ids() {
        let (_dir, mut env) = env_with_token();
        env.insert("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), "7,bad".to_string());
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("comma-separated integers"));

        let (_dir, mut env) = env_with_token();
        env.insert("GATEWAY_TELEGRAM_CHAT_IDS".to_string(), " , ".to_string());
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("at least one id"));

        let (_dir, mut env) = env_with_token();
        env.insert(
            "GATEWAY_TELEGRAM_CHAT_IDS".to_string(),
            "42,-100".to_string(),
        );
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("private chat ids"));
    }

    #[test]
    fn telegram_chat_ids_are_trimmed_sorted_and_deduplicated() {
        let (_dir, mut env) = env_with_token();
        env.insert(
            "GATEWAY_TELEGRAM_CHAT_IDS".to_string(),
            " 9, 7,9 ".to_string(),
        );

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.telegram_chat_ids, vec![7, 9]);
    }

    #[test]
    fn gateway_config_creates_default_when_missing_and_rejects_incomplete_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");

        let cfg = load_gateway_config(&path).unwrap();

        assert_eq!(cfg, GatewayConfigFile::default());
        assert!(path.exists());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{}").unwrap();
        let err = load_gateway_config(&path).unwrap_err();
        assert!(err.contains("parse gateway config"));

        fs::write(&path, "{").unwrap();
        let err = load_gateway_config(&path).unwrap_err();
        assert!(err.contains("parse gateway config"));
    }

    #[test]
    fn zero_timeout_and_overflow_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":0}"#,
        )
        .unwrap();
        let err = load_gateway_config(&path).unwrap_err();
        assert!(err.contains("timeout_mins must be greater than zero"));

        let (_dir, env) = env_with_token();
        let path = PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            format!(
                r#"{{"models":[{{"provider":"codex","model":"gpt-test"}}],"timeout_mins":{}}}"#,
                u64::MAX
            ),
        )
        .unwrap();
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("timeout_mins is too large"));
    }

    #[test]
    fn current_env_reads_process_environment() {
        let env = current_env();

        assert!(!env.is_empty());
    }

    #[test]
    fn save_gateway_config_reports_parent_creation_errors() {
        let dir = tempfile::tempdir().unwrap();
        let blocked_parent = dir.path().join("blocked");
        fs::write(&blocked_parent, "file").unwrap();

        let err = save_gateway_config(
            &blocked_parent.join("config.json"),
            &GatewayConfigFile::default(),
        )
        .unwrap_err();

        assert!(err.contains("create gateway config dir"));
    }
}
