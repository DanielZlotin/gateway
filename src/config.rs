use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TELEGRAM_BOT_TOKEN_ENV: &str = "TELEGRAM_BOT_TOKEN";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

const DEFAULT_FASTFETCH_ARGS: &[&str] = &[
    "--logo",
    "none",
    "--pipe",
    "--structure",
    "OS:Host:Kernel:Uptime:CPU:GPU:Memory:Swap:Disk:Battery:LocalIp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bot_token: String,
    pub allowed_ids: Vec<i64>,
    pub user_home: PathBuf,
    pub path: String,
    pub xdg_config_home: PathBuf,
    pub xdg_cache_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub gateway_config_file: PathBuf,
    pub codex_bin: PathBuf,
    pub codex_home: PathBuf,
    pub codex_workdir: PathBuf,
    pub codex_model: String,
    pub fastfetch_bin: PathBuf,
    pub fastfetch_args: Vec<String>,
    pub state_dir: PathBuf,
    pub chat_state_dir: PathBuf,
    pub cron_state_dir: PathBuf,
    pub offset_file: PathBuf,
    pub gateway_log_file: PathBuf,
    pub launchd_target: String,
    pub poll_timeout_sec: u64,
    pub queue_depth: usize,
    pub codex_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayConfigFile {
    #[serde(default = "default_codex_model")]
    pub model: String,
    #[serde(default)]
    pub fastfetch: FastfetchConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FastfetchConfig {
    #[serde(default = "default_fastfetch_args")]
    pub args: Vec<String>,
}

pub fn current_env() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

pub fn load() -> Result<Config, String> {
    load_from_env(&current_env())
}

pub fn load_from_env(env: &BTreeMap<String, String>) -> Result<Config, String> {
    let bot_token = required(env, TELEGRAM_BOT_TOKEN_ENV)?;
    let user_home = required_path(env, "HOME")?;
    let path_value = required(env, "PATH")?;
    let xdg_config_home = required_path(env, "XDG_CONFIG_HOME")?;
    let xdg_cache_home = required_path(env, "XDG_CACHE_HOME")?;
    let xdg_data_home = required_path(env, "XDG_DATA_HOME")?;
    let xdg_state_home = required_path(env, "XDG_STATE_HOME")?;
    let gateway_config_file = path(
        env,
        "GATEWAY_CONFIG_FILE",
        xdg_config_home.join("gateway/config.json"),
    );
    let gateway_config = load_gateway_config(&gateway_config_file)?;
    let state_dir = path(env, "GATEWAY_STATE_DIR", xdg_state_home.join("gateway"));
    let chat_state_dir = state_dir.join("chats");
    let cron_state_dir = state_dir.join("cron");

    Ok(Config {
        bot_token,
        allowed_ids: allowed_ids(env)?,
        user_home,
        path: path_value,
        xdg_config_home: xdg_config_home.clone(),
        xdg_cache_home,
        xdg_data_home,
        xdg_state_home: xdg_state_home.clone(),
        gateway_config_file,
        codex_bin: path(
            env,
            "GATEWAY_CODEX_BIN",
            PathBuf::from("/opt/homebrew/bin/codex"),
        ),
        codex_home: path(env, "GATEWAY_CODEX_HOME", xdg_config_home.join("codex")),
        codex_workdir: path(env, "GATEWAY_CODEX_WORKDIR", xdg_config_home),
        codex_model: gateway_config.model,
        fastfetch_bin: path(
            env,
            "GATEWAY_FASTFETCH_BIN",
            PathBuf::from("/opt/homebrew/bin/fastfetch"),
        ),
        fastfetch_args: gateway_config.fastfetch.args,
        state_dir: state_dir.clone(),
        chat_state_dir,
        cron_state_dir,
        offset_file: state_dir.join("telegram.offset"),
        gateway_log_file: path(env, "GATEWAY_LOG_FILE", state_dir.join("logs/gateway.log")),
        launchd_target: value(env, "GATEWAY_LAUNCHD_TARGET", "ai.gateway"),
        poll_timeout_sec: number(env, "GATEWAY_POLL_TIMEOUT_SECS", 50)?,
        queue_depth: number(env, "GATEWAY_QUEUE_DEPTH", 8)?,
        codex_timeout: Duration::from_secs(number(env, "GATEWAY_CODEX_TIMEOUT_SECS", 45 * 60)?),
    })
}

pub fn load_gateway_config(path: &Path) -> Result<GatewayConfigFile, String> {
    if !path.exists() {
        let cfg = GatewayConfigFile::default();
        save_gateway_config(path, &cfg)?;
        return Ok(cfg);
    }

    let text = fs::read_to_string(path).map_err(|err| format!("read gateway config: {err}"))?;
    let mut cfg: GatewayConfigFile =
        serde_json::from_str(&text).map_err(|err| format!("parse gateway config: {err}"))?;
    cfg.normalize();
    save_gateway_config(path, &cfg)?;
    Ok(cfg)
}

pub fn save_gateway_config(path: &Path, cfg: &GatewayConfigFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create gateway config dir: {err}"))?;
    }
    let data = serde_json::to_vec_pretty(cfg).map_err(|err| err.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, [data, b"\n".to_vec()].concat())
        .map_err(|err| format!("write gateway config: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("replace gateway config: {err}"))
}

fn required(env: &BTreeMap<String, String>, key: &str) -> Result<String, String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{key} is required"))
}

fn value(env: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn required_path(env: &BTreeMap<String, String>, key: &str) -> Result<PathBuf, String> {
    required(env, key).map(PathBuf::from)
}

fn path(env: &BTreeMap<String, String>, key: &str, default: PathBuf) -> PathBuf {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

fn number<T>(env: &BTreeMap<String, String>, key: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + Copy,
{
    match env
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(value) => value
            .parse::<T>()
            .map_err(|_| format!("{key} must be a number")),
        None => Ok(default),
    }
}

fn allowed_ids(env: &BTreeMap<String, String>) -> Result<Vec<i64>, String> {
    let raw = required(env, "GATEWAY_ALLOWED_IDS")?;
    let mut ids = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        ids.push(part.parse::<i64>().map_err(|_| {
            "GATEWAY_ALLOWED_IDS must contain comma-separated integers".to_string()
        })?);
    }
    if ids.is_empty() {
        return Err("GATEWAY_ALLOWED_IDS must include at least one id".to_string());
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

impl Default for GatewayConfigFile {
    fn default() -> Self {
        Self {
            model: default_codex_model(),
            fastfetch: FastfetchConfig::default(),
        }
    }
}

impl GatewayConfigFile {
    pub fn normalize(&mut self) {
        if self.model.trim().is_empty() {
            self.model = default_codex_model();
        } else {
            self.model = self.model.trim().to_string();
        }
        self.fastfetch.normalize();
    }
}

impl Default for FastfetchConfig {
    fn default() -> Self {
        Self {
            args: default_fastfetch_args(),
        }
    }
}

impl FastfetchConfig {
    fn normalize(&mut self) {
        self.args = self
            .args
            .iter()
            .map(|arg| arg.trim())
            .filter(|arg| !arg.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        if self.args.is_empty() {
            self.args = default_fastfetch_args();
        }
    }
}

fn default_codex_model() -> String {
    DEFAULT_CODEX_MODEL.to_string()
}

fn default_fastfetch_args() -> Vec<String> {
    DEFAULT_FASTFETCH_ARGS
        .iter()
        .map(|arg| (*arg).to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with_token() -> (tempfile::TempDir, BTreeMap<String, String>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let env = BTreeMap::from([
            (TELEGRAM_BOT_TOKEN_ENV.to_string(), "token".to_string()),
            ("GATEWAY_ALLOWED_IDS".to_string(), "42".to_string()),
            ("HOME".to_string(), "/home/example".to_string()),
            ("PATH".to_string(), "/bin:/usr/bin".to_string()),
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
    fn loads_defaults_from_env_and_creates_config() {
        let (_dir, env) = env_with_token();
        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.bot_token, "token");
        assert_eq!(cfg.allowed_ids, vec![42]);
        assert_eq!(cfg.user_home, PathBuf::from("/home/example"));
        assert_eq!(cfg.path, "/bin:/usr/bin");
        assert!(cfg.xdg_config_home.ends_with("config"));
        assert!(cfg.xdg_cache_home.ends_with("cache"));
        assert!(cfg.xdg_data_home.ends_with("data"));
        assert!(cfg.xdg_state_home.ends_with("state"));
        assert_eq!(cfg.codex_bin, PathBuf::from("/opt/homebrew/bin/codex"));
        assert_eq!(cfg.codex_home, cfg.xdg_config_home.join("codex"));
        assert_eq!(cfg.codex_workdir, cfg.xdg_config_home);
        assert_eq!(cfg.state_dir, cfg.xdg_state_home.join("gateway"));
        assert_eq!(cfg.gateway_log_file, cfg.state_dir.join("logs/gateway.log"));
        assert_eq!(
            cfg.gateway_config_file,
            cfg.xdg_config_home.join("gateway/config.json")
        );
        assert!(cfg.gateway_config_file.exists());
        assert_eq!(cfg.launchd_target, "ai.gateway");
        assert_eq!(cfg.codex_model, DEFAULT_CODEX_MODEL);
        assert_eq!(cfg.fastfetch_args, default_fastfetch_args());
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(45 * 60));
    }

    #[test]
    fn rejects_missing_bot_token() {
        let err = load_from_env(&BTreeMap::new()).unwrap_err();
        assert!(err.contains(TELEGRAM_BOT_TOKEN_ENV));
    }

    #[test]
    fn rejects_missing_allowed_ids() {
        let (_dir, mut env) = env_with_token();
        env.remove("GATEWAY_ALLOWED_IDS");

        let err = load_from_env(&env).unwrap_err();

        assert!(err.contains("GATEWAY_ALLOWED_IDS"));
    }

    #[test]
    fn parses_overrides() {
        let (_dir, mut env) = env_with_token();
        env.insert("GATEWAY_ALLOWED_IDS".to_string(), "7,8".to_string());
        env.insert("GATEWAY_QUEUE_DEPTH".to_string(), "3".to_string());
        env.insert("GATEWAY_CODEX_TIMEOUT_SECS".to_string(), "9".to_string());
        env.insert("GATEWAY_STATE_DIR".to_string(), "/tmp/gateway".to_string());
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        save_gateway_config(
            &cfg_path,
            &GatewayConfigFile {
                model: "gpt-test".to_string(),
                fastfetch: FastfetchConfig {
                    args: vec!["--pipe".to_string()],
                },
            },
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.allowed_ids, vec![7, 8]);
        assert_eq!(cfg.codex_model, "gpt-test");
        assert_eq!(cfg.fastfetch_args, vec!["--pipe"]);
        assert_eq!(cfg.queue_depth, 3);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(9));
        assert_eq!(cfg.chat_state_dir, PathBuf::from("/tmp/gateway/chats"));
        assert_eq!(cfg.cron_state_dir, PathBuf::from("/tmp/gateway/cron"));
    }

    #[test]
    fn normalizes_gateway_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"model":"  ","fastfetch":{"args":[" ","--pipe"]}}"#,
        )
        .unwrap();

        let cfg = load_gateway_config(&path).unwrap();

        assert_eq!(cfg.model, DEFAULT_CODEX_MODEL);
        assert_eq!(cfg.fastfetch.args, vec!["--pipe"]);
    }
}
