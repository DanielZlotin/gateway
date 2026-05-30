use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

pub const TELEGRAM_BOT_TOKEN_ENV: &str = "TELEGRAM_BOT_TOKEN";
pub const DEFAULT_ALLOWED_ID: i64 = <telegram_chat_id>;
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub bot_token: String,
    pub allowed_ids: Vec<i64>,
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
    let state_dir = path(
        env,
        "GATEWAY_STATE_DIR",
        "/Users/example/.local/state/gateway",
    );
    let chat_state_dir = state_dir.join("chats");
    let cron_state_dir = state_dir.join("cron");

    Ok(Config {
        bot_token,
        allowed_ids: allowed_ids(env)?,
        codex_bin: path(env, "GATEWAY_CODEX_BIN", "/opt/homebrew/bin/codex"),
        codex_home: path(env, "GATEWAY_CODEX_HOME", "/Users/example/.config/codex"),
        codex_workdir: path(env, "GATEWAY_CODEX_WORKDIR", "/Users/example/.config"),
        codex_model: value(env, "GATEWAY_CODEX_MODEL", DEFAULT_CODEX_MODEL),
        fastfetch_bin: path(env, "GATEWAY_FASTFETCH_BIN", "/opt/homebrew/bin/fastfetch"),
        state_dir: state_dir.clone(),
        chat_state_dir,
        cron_state_dir,
        offset_file: state_dir.join("telegram.offset"),
        gateway_log_file: path(
            env,
            "GATEWAY_LOG_FILE",
            "/Users/example/.local/share/gateway/logs/gateway.log",
        ),
        launchd_target: value(env, "GATEWAY_LAUNCHD_TARGET", "gui/<uid>/ai.gateway"),
        poll_timeout_sec: number(env, "GATEWAY_POLL_TIMEOUT_SECS", 50)?,
        queue_depth: number(env, "GATEWAY_QUEUE_DEPTH", 8)?,
        codex_timeout: Duration::from_secs(number(
            env,
            "GATEWAY_CODEX_TIMEOUT_SECS",
            45 * 60,
        )?),
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

fn path(env: &BTreeMap<String, String>, key: &str, default: &str) -> PathBuf {
    PathBuf::from(value(env, key, default))
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
    let Some(raw) = env.get("GATEWAY_ALLOWED_IDS") else {
        return Ok(vec![DEFAULT_ALLOWED_ID]);
    };
    let mut ids = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|part| !part.is_empty()) {
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
        BTreeMap::from([(TELEGRAM_BOT_TOKEN_ENV.to_string(), "token".to_string())])
    }

    #[test]
    fn loads_defaults_from_env() {
        let cfg = load_from_env(&env_with_token()).unwrap();

        assert_eq!(cfg.bot_token, "token");
        assert_eq!(cfg.allowed_ids, vec![DEFAULT_ALLOWED_ID]);
        assert_eq!(cfg.codex_bin, PathBuf::from("/opt/homebrew/bin/codex"));
        assert_eq!(
            cfg.codex_home,
            PathBuf::from("/Users/example/.config/codex")
        );
        assert_eq!(cfg.codex_workdir, PathBuf::from("/Users/example/.config"));
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
