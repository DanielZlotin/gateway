use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub const TELEGRAM_BOT_TOKEN_ENV: &str = "TELEGRAM_BOT_TOKEN";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

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
    pub codex_bin: PathBuf,
    pub codex_home: PathBuf,
    pub codex_workdir: PathBuf,
    pub codex_model: String,
    pub fastfetch_bin: PathBuf,
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
        codex_bin: path(
            env,
            "GATEWAY_CODEX_BIN",
            PathBuf::from("/opt/homebrew/bin/codex"),
        ),
        codex_home: path(env, "GATEWAY_CODEX_HOME", xdg_config_home.join("codex")),
        codex_workdir: path(env, "GATEWAY_CODEX_WORKDIR", xdg_config_home),
        codex_model: value(env, "GATEWAY_CODEX_MODEL", DEFAULT_CODEX_MODEL),
        fastfetch_bin: path(
            env,
            "GATEWAY_FASTFETCH_BIN",
            PathBuf::from("/opt/homebrew/bin/fastfetch"),
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with_token() -> BTreeMap<String, String> {
        BTreeMap::from([
            (TELEGRAM_BOT_TOKEN_ENV.to_string(), "token".to_string()),
            ("GATEWAY_ALLOWED_IDS".to_string(), "42".to_string()),
            ("HOME".to_string(), "/home/example".to_string()),
            ("PATH".to_string(), "/bin:/usr/bin".to_string()),
            ("XDG_CONFIG_HOME".to_string(), "/xdg/config".to_string()),
            ("XDG_CACHE_HOME".to_string(), "/xdg/cache".to_string()),
            ("XDG_DATA_HOME".to_string(), "/xdg/data".to_string()),
            ("XDG_STATE_HOME".to_string(), "/xdg/state".to_string()),
        ])
    }

    #[test]
    fn loads_defaults_from_env() {
        let cfg = load_from_env(&env_with_token()).unwrap();

        assert_eq!(cfg.bot_token, "token");
        assert_eq!(cfg.allowed_ids, vec![42]);
        assert_eq!(cfg.user_home, PathBuf::from("/home/example"));
        assert_eq!(cfg.path, "/bin:/usr/bin");
        assert_eq!(cfg.xdg_config_home, PathBuf::from("/xdg/config"));
        assert_eq!(cfg.xdg_cache_home, PathBuf::from("/xdg/cache"));
        assert_eq!(cfg.xdg_data_home, PathBuf::from("/xdg/data"));
        assert_eq!(cfg.xdg_state_home, PathBuf::from("/xdg/state"));
        assert_eq!(cfg.codex_bin, PathBuf::from("/opt/homebrew/bin/codex"));
        assert_eq!(cfg.codex_home, PathBuf::from("/xdg/config/codex"));
        assert_eq!(cfg.codex_workdir, PathBuf::from("/xdg/config"));
        assert_eq!(cfg.state_dir, PathBuf::from("/xdg/state/gateway"));
        assert_eq!(
            cfg.gateway_log_file,
            PathBuf::from("/xdg/state/gateway/logs/gateway.log")
        );
        assert_eq!(cfg.launchd_target, "ai.gateway");
        assert_eq!(cfg.codex_model, DEFAULT_CODEX_MODEL);
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
        let mut env = env_with_token();
        env.remove("GATEWAY_ALLOWED_IDS");

        let err = load_from_env(&env).unwrap_err();

        assert!(err.contains("GATEWAY_ALLOWED_IDS"));
    }

    #[test]
    fn parses_overrides() {
        let mut env = env_with_token();
        env.insert("GATEWAY_ALLOWED_IDS".to_string(), "7,8".to_string());
        env.insert("GATEWAY_CODEX_MODEL".to_string(), "gpt-test".to_string());
        env.insert("GATEWAY_QUEUE_DEPTH".to_string(), "3".to_string());
        env.insert("GATEWAY_CODEX_TIMEOUT_SECS".to_string(), "9".to_string());
        env.insert("GATEWAY_STATE_DIR".to_string(), "/tmp/gateway".to_string());

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.allowed_ids, vec![7, 8]);
        assert_eq!(cfg.codex_model, "gpt-test");
        assert_eq!(cfg.queue_depth, 3);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(9));
        assert_eq!(cfg.chat_state_dir, PathBuf::from("/tmp/gateway/chats"));
        assert_eq!(cfg.cron_state_dir, PathBuf::from("/tmp/gateway/cron"));
    }
}
