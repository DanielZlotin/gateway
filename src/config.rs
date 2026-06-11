use crate::json_file::{save_pretty_json, SaveJsonLabels};
use crate::launchd;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const GATEWAY_TELEGRAM_TOKEN_ENV: &str = "GATEWAY_TELEGRAM_TOKEN";
pub const GATEWAY_TELEGRAM_CHAT_ID_ENV: &str = "GATEWAY_TELEGRAM_CHAT_ID";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";
pub const DEFAULT_LIGHT_CODEX_MODEL: &str = "gpt-5.4-mini";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-4-8";
pub const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-5.5";
pub const DEFAULT_CODEX_TIMEOUT_MINS: u64 = 30;
pub const DEFAULT_HEARTBEAT: &str = "1d";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub bot_token: String,
    pub telegram_chat_ids: Vec<i64>,
    pub telegram_bots: Vec<TelegramBotConfig>,
    pub xdg_config_home: PathBuf,
    pub xdg_cache_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub gateway_config_file: PathBuf,
    pub codex_workdir: PathBuf,
    pub models: Vec<ProviderModel>,
    pub tts: Option<serde_json::Value>,
    pub state_dir: PathBuf,
    pub chat_state_dir: PathBuf,
    pub offset_file: PathBuf,
    pub gateway_log_file: PathBuf,
    pub launchd_target: String,
    pub poll_timeout_sec: u64,
    pub queue_depth: usize,
    pub codex_timeout: Duration,
    pub heartbeat_interval: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfiguredTts {
    ElevenLabs {
        model: String,
        voice: String,
        speed: Option<f64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramBotConfig {
    pub bot_token: String,
    pub chat_ids: Vec<i64>,
    pub offset_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfigFile {
    pub models: Vec<ProviderModel>,
    #[serde(default = "default_timeout_mins")]
    pub timeout_mins: u64,
    #[serde(default = "default_heartbeat")]
    pub heartbeat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    pub provider: Provider,
    pub model: String,
    #[serde(default, skip_serializing_if = "ModelRole::is_default")]
    pub role: ModelRole,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    #[default]
    Default,
    Light,
}

impl ModelRole {
    pub const fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Light => "light",
        }
    }
}

pub fn current_env() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

pub fn load() -> Result<Config, String> {
    load_from_env(&current_env())
}

pub fn load_from_env(env: &BTreeMap<String, String>) -> Result<Config, String> {
    let bot_tokens = telegram_tokens(env)?;
    let xdg_config_home = resolve_xdg_config_home(env)?;
    let xdg_cache_home = resolve_xdg_cache_home(env)?;
    let xdg_data_home = resolve_xdg_data_home(env)?;
    let xdg_state_home = resolve_xdg_state_home(env)?;
    let gateway_config_file = xdg_config_home.join("gateway/config.json");
    let gateway_config = load_gateway_config(&gateway_config_file)?;
    let heartbeat_interval = gateway_config.heartbeat_interval()?;
    let state_dir = xdg_state_home.join("gateway");
    let chat_state_dir = state_dir.join("chats");
    let launchd_target = launchd::target()?;
    let telegram_chat_ids_ordered = telegram_chat_ids(env)?;
    let telegram_chat_ids = sorted_unique_ids(&telegram_chat_ids_ordered);
    let telegram_bots = telegram_bots(&bot_tokens, &telegram_chat_ids_ordered, &state_dir)?;

    Ok(Config {
        bot_token: bot_tokens[0].clone(),
        telegram_chat_ids,
        telegram_bots,
        xdg_config_home: xdg_config_home.clone(),
        xdg_cache_home,
        xdg_data_home,
        xdg_state_home,
        gateway_config_file,
        codex_workdir: path(env, "GATEWAY_CODEX_WORKDIR", xdg_config_home),
        models: gateway_config.models,
        tts: gateway_config.tts,
        state_dir: state_dir.clone(),
        chat_state_dir,
        offset_file: state_dir.join("telegram.offset"),
        gateway_log_file: state_dir.join("logs/gateway.log"),
        launchd_target,
        poll_timeout_sec: 50,
        queue_depth: 8,
        codex_timeout: Duration::from_secs(timeout_secs(gateway_config.timeout_mins)?),
        heartbeat_interval,
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

fn telegram_tokens(env: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let raw = required(env, GATEWAY_TELEGRAM_TOKEN_ENV)?;
    let tokens = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(format!(
            "{GATEWAY_TELEGRAM_TOKEN_ENV} must include at least one token"
        ));
    }
    Ok(tokens)
}

fn telegram_chat_ids(env: &BTreeMap<String, String>) -> Result<Vec<i64>, String> {
    let raw = required(env, GATEWAY_TELEGRAM_CHAT_ID_ENV)?;
    let mut ids = Vec::new();
    for part in raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let id = part.parse::<i64>().map_err(|_| {
            format!("{GATEWAY_TELEGRAM_CHAT_ID_ENV} must contain comma-separated integers")
        })?;
        if id <= 0 {
            return Err(format!(
                "{GATEWAY_TELEGRAM_CHAT_ID_ENV} must contain private chat ids only"
            ));
        }
        ids.push(id);
    }
    if ids.is_empty() {
        return Err(format!(
            "{GATEWAY_TELEGRAM_CHAT_ID_ENV} must include at least one id"
        ));
    }
    Ok(ids)
}

fn sorted_unique_ids(ids: &[i64]) -> Vec<i64> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn telegram_bots(
    bot_tokens: &[String],
    chat_ids: &[i64],
    state_dir: &Path,
) -> Result<Vec<TelegramBotConfig>, String> {
    if bot_tokens.len() == 1 {
        return Ok(vec![TelegramBotConfig {
            bot_token: bot_tokens[0].clone(),
            chat_ids: sorted_unique_ids(chat_ids),
            offset_file: state_dir.join("telegram.offset"),
        }]);
    }
    if bot_tokens.len() != chat_ids.len() {
        return Err(format!(
            "{GATEWAY_TELEGRAM_TOKEN_ENV} and {GATEWAY_TELEGRAM_CHAT_ID_ENV} must contain the same number of comma-separated values when multiple bot tokens are configured"
        ));
    }
    Ok(bot_tokens
        .iter()
        .zip(chat_ids.iter())
        .enumerate()
        .map(|(index, (bot_token, chat_id))| TelegramBotConfig {
            bot_token: bot_token.clone(),
            chat_ids: vec![*chat_id],
            offset_file: state_dir.join(format!("telegram-{}.offset", index + 1)),
        })
        .collect())
}

impl Default for GatewayConfigFile {
    fn default() -> Self {
        Self {
            models: default_models(),
            timeout_mins: default_timeout_mins(),
            heartbeat: default_heartbeat(),
            tts: None,
        }
    }
}

impl GatewayConfigFile {
    pub fn normalize(&mut self) -> Result<(), String> {
        normalize_models(&mut self.models)?;
        if self.timeout_mins == 0 {
            return Err("timeout_mins must be greater than zero".to_string());
        }
        self.heartbeat = self.heartbeat.trim().to_ascii_lowercase();
        self.heartbeat_interval()?;
        Ok(())
    }

    pub fn heartbeat_interval(&self) -> Result<Duration, String> {
        heartbeat_interval(&self.heartbeat)
    }
}

pub fn default_models() -> Vec<ProviderModel> {
    vec![
        ProviderModel {
            provider: Provider::Codex,
            model: DEFAULT_CODEX_MODEL.to_string(),
            role: ModelRole::Default,
        },
        ProviderModel {
            provider: Provider::Codex,
            model: DEFAULT_LIGHT_CODEX_MODEL.to_string(),
            role: ModelRole::Light,
        },
        ProviderModel {
            provider: Provider::Claude,
            model: DEFAULT_CLAUDE_MODEL.to_string(),
            role: ModelRole::Default,
        },
        ProviderModel {
            provider: Provider::Openrouter,
            model: DEFAULT_OPENROUTER_MODEL.to_string(),
            role: ModelRole::Default,
        },
    ]
}

const fn default_timeout_mins() -> u64 {
    DEFAULT_CODEX_TIMEOUT_MINS
}

fn default_heartbeat() -> String {
    DEFAULT_HEARTBEAT.to_string()
}

fn timeout_secs(timeout_mins: u64) -> Result<u64, String> {
    timeout_mins
        .checked_mul(60)
        .ok_or_else(|| "timeout_mins is too large".to_string())
}

fn heartbeat_interval(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.len() < 2 {
        return Err("heartbeat must use a positive duration like 1m, 3h, or 1d".to_string());
    }
    let (number, unit) = value.split_at(value.len() - 1);
    let count = number
        .parse::<u64>()
        .map_err(|_| "heartbeat must use a positive duration like 1m, 3h, or 1d".to_string())?;
    if count == 0 {
        return Err("heartbeat must be greater than zero".to_string());
    }
    let seconds_per_unit = match unit {
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => return Err("heartbeat unit must be m, h, or d".to_string()),
    };
    count
        .checked_mul(seconds_per_unit)
        .map(Duration::from_secs)
        .ok_or_else(|| "heartbeat is too large".to_string())
}

impl Config {
    pub fn bot_token_for_chat(&self, chat_id: i64) -> Option<&str> {
        self.telegram_bots
            .iter()
            .find(|bot| bot.chat_ids.contains(&chat_id))
            .map(|bot| bot.bot_token.as_str())
            .or_else(|| {
                self.telegram_chat_ids
                    .contains(&chat_id)
                    .then_some(self.bot_token.as_str())
            })
    }

    pub fn default_provider_model(&self) -> &ProviderModel {
        self.models
            .iter()
            .find(|model| model.role == ModelRole::Default)
            .or_else(|| self.models.first())
            .expect("gateway config normalization ensures at least one model")
    }

    pub fn light_provider_model(&self) -> &ProviderModel {
        self.models
            .iter()
            .find(|model| model.role == ModelRole::Light)
            .unwrap_or_else(|| self.default_provider_model())
    }

    pub fn provider_model_at(&self, index: usize) -> Option<&ProviderModel> {
        self.models.get(index)
    }

    pub fn tts_fallback_warning(&self) -> Option<String> {
        self.configured_tts().err().map(|err| {
            format!(
                "⚠️ Invalid `tts` config in {}: {err}; falling back to local Voicebox.",
                self.gateway_config_file.display()
            )
        })
    }

    pub fn configured_tts(&self) -> Result<Option<ConfiguredTts>, String> {
        let Some(value) = self.tts.as_ref() else {
            return Ok(None);
        };
        configured_tts(value).map(Some)
    }
}

fn configured_tts(value: &serde_json::Value) -> Result<ConfiguredTts, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "`tts` must be an object".to_string())?;
    let provider =
        tts_required_string(object, "provider", "`tts.provider` must be \"elevenlabs\"")?;
    if !provider.eq_ignore_ascii_case("elevenlabs") {
        return Err(format!(
            "unsupported `tts.provider` \"{provider}\"; expected \"elevenlabs\""
        ));
    }
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "provider" | "model" | "voice" | "speed"))
    {
        return Err(format!(
            "unsupported `tts.{key}` field; expected `provider`, `model`, `voice`, or `speed`"
        ));
    }
    let model = tts_required_string(object, "model", "`tts.model` must be a non-empty string")?;
    let voice = tts_required_string(object, "voice", "`tts.voice` must be a non-empty string")?;
    let speed = tts_optional_speed(object)?;
    Ok(ConfiguredTts::ElevenLabs {
        model,
        voice,
        speed,
    })
}

fn tts_required_string(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    message: &str,
) -> Result<String, String> {
    object
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| message.to_string())
}

fn tts_optional_speed(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<f64>, String> {
    let Some(value) = object.get("speed") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .filter(|speed| speed.is_finite() && *speed > 0.0)
        .map(Some)
        .ok_or_else(|| "`tts.speed` must be a positive number".to_string())
}

fn normalize_models(models: &mut Vec<ProviderModel>) -> Result<(), String> {
    for item in models.iter_mut() {
        item.model = item.model.trim().to_string();
    }
    models.retain(|item| !item.model.is_empty());
    if models.is_empty() {
        return Err("gateway config must include at least one non-empty model".to_string());
    }
    if !models.iter().any(|item| item.role == ModelRole::Light) {
        models.push(ProviderModel {
            provider: Provider::Codex,
            model: DEFAULT_LIGHT_CODEX_MODEL.to_string(),
            role: ModelRole::Light,
        });
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
            (GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "42".to_string()),
            (
                "HOME".to_string(),
                root.join("home").to_string_lossy().to_string(),
            ),
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
                    model: DEFAULT_CODEX_MODEL.to_string(),
                    role: ModelRole::Default,
                },
                ProviderModel {
                    provider: Provider::Codex,
                    model: DEFAULT_LIGHT_CODEX_MODEL.to_string(),
                    role: ModelRole::Light,
                },
                ProviderModel {
                    provider: Provider::Claude,
                    model: "claude-opus-4-8".to_string(),
                    role: ModelRole::Default,
                },
                ProviderModel {
                    provider: Provider::Openrouter,
                    model: DEFAULT_OPENROUTER_MODEL.to_string(),
                    role: ModelRole::Default,
                }
            ]
        );
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(30 * 60));

        let text = fs::read_to_string(&cfg.gateway_config_file).unwrap();
        assert!(text.contains(r#""models""#));
        assert!(text.contains(r#""role": "light""#));
        assert!(!text.contains(r#""tts""#));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("provider").is_none());
        assert!(!text.contains(r#""claude_model""#));
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
            (GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "42".to_string()),
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
            (GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "42".to_string()),
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
        env.remove(GATEWAY_TELEGRAM_CHAT_ID_ENV);

        let err = load_from_env(&env).unwrap_err();

        assert!(err.contains(GATEWAY_TELEGRAM_CHAT_ID_ENV));
    }

    #[test]
    fn parses_supported_overrides() {
        let (_dir, mut env) = env_with_token();
        env.insert(GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "7,8".to_string());
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
                model: "gpt-test".to_string(),
                role: ModelRole::Default,
            }
        );
        assert_eq!(cfg.light_provider_model().model, DEFAULT_LIGHT_CODEX_MODEL);
        assert_eq!(cfg.queue_depth, 8);
        assert_eq!(cfg.codex_timeout, Duration::from_secs(9 * 60));
        assert_eq!(cfg.state_dir, cfg.xdg_state_home.join("gateway"));
        assert_eq!(cfg.gateway_log_file, cfg.state_dir.join("logs/gateway.log"));
        assert_eq!(cfg.poll_timeout_sec, 50);
        assert!(cfg.launchd_target.starts_with("gui/"));
        assert!(cfg.launchd_target.ends_with("/ai.gateway"));
    }

    #[test]
    fn parses_multiple_telegram_bots_from_aligned_tokens_and_chat_ids() {
        let (dir, mut env) = env_with_token();
        env.insert(
            GATEWAY_TELEGRAM_TOKEN_ENV.to_string(),
            " token-a , token-b ".to_string(),
        );
        env.insert(
            GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(),
            "77,42".to_string(),
        );

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.bot_token, "token-a");
        assert_eq!(cfg.telegram_chat_ids, vec![42, 77]);
        assert_eq!(
            cfg.telegram_bots,
            vec![
                TelegramBotConfig {
                    bot_token: "token-a".to_string(),
                    chat_ids: vec![77],
                    offset_file: dir.path().join("state/gateway/telegram-1.offset"),
                },
                TelegramBotConfig {
                    bot_token: "token-b".to_string(),
                    chat_ids: vec![42],
                    offset_file: dir.path().join("state/gateway/telegram-2.offset"),
                },
            ]
        );
        assert_eq!(cfg.bot_token_for_chat(77), Some("token-a"));
        assert_eq!(cfg.bot_token_for_chat(42), Some("token-b"));
    }

    #[test]
    fn rejects_mismatched_multiple_telegram_tokens_and_chat_ids() {
        let (_dir, mut env) = env_with_token();
        env.insert(
            GATEWAY_TELEGRAM_TOKEN_ENV.to_string(),
            "token-a,token-b".to_string(),
        );
        env.insert(GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "42".to_string());

        let err = load_from_env(&env).unwrap_err();

        assert!(err.contains(GATEWAY_TELEGRAM_TOKEN_ENV));
        assert!(err.contains(GATEWAY_TELEGRAM_CHAT_ID_ENV));
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
            vec![
                ProviderModel {
                    provider: Provider::Claude,
                    model: "claude-test".to_string(),
                    role: ModelRole::Default,
                },
                ProviderModel {
                    provider: Provider::Codex,
                    model: DEFAULT_LIGHT_CODEX_MODEL.to_string(),
                    role: ModelRole::Light,
                },
            ]
        );
        let text = fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(value.get("provider").is_none());
        assert!(text.contains("\"timeout_mins\": 30"));
    }

    #[test]
    fn model_roles_select_first_default_and_first_light() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"codex","model":"gpt-light","role":"light"},{"provider":"claude","model":"claude-default"},{"provider":"codex","model":"gpt-other","role":"default"},{"provider":"codex","model":"gpt-light-2","role":"light"}],"timeout_mins":30}"#,
        )
        .unwrap();
        let env = {
            let mut env = BTreeMap::new();
            env.insert(GATEWAY_TELEGRAM_TOKEN_ENV.to_string(), "token".to_string());
            env.insert(GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), "42".to_string());
            env.insert(
                "XDG_CONFIG_HOME".to_string(),
                dir.path().to_string_lossy().to_string(),
            );
            env.insert(
                "HOME".to_string(),
                dir.path().join("home").to_string_lossy().to_string(),
            );
            env
        };

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.default_provider_model().model, "claude-default");
        assert_eq!(cfg.light_provider_model().model, "gpt-light");
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(
            r#""model": "claude-default",
      "role""#
        ));
        assert!(text.contains(r#""role": "light""#));
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
    fn gateway_config_defaults_missing_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway/config.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}]}"#,
        )
        .unwrap();

        let cfg = load_gateway_config(&path).unwrap();

        assert_eq!(cfg.heartbeat, "1d");
        assert_eq!(
            cfg.heartbeat_interval().unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"heartbeat\": \"1d\""));
    }

    #[test]
    fn parses_valid_heartbeat_durations() {
        for (heartbeat, seconds) in [
            ("1m", 60),
            ("3m", 180),
            ("1h", 60 * 60),
            ("3h", 3 * 60 * 60),
            ("1d", 24 * 60 * 60),
            ("7d", 7 * 24 * 60 * 60),
        ] {
            let cfg = gateway_config_with_heartbeat(heartbeat);

            assert_eq!(
                cfg.heartbeat_interval().unwrap(),
                Duration::from_secs(seconds),
                "heartbeat={heartbeat}"
            );
        }
    }

    #[test]
    fn rejects_invalid_heartbeat_durations() {
        for heartbeat in ["", " ", "0m", "1", "m", "3x", "-1h"] {
            let mut cfg = gateway_config_with_heartbeat(heartbeat);

            let err = cfg.normalize().unwrap_err();

            assert!(
                err.contains("heartbeat"),
                "heartbeat={heartbeat:?} error={err}"
            );
        }
    }

    fn gateway_config_with_heartbeat(heartbeat: &str) -> GatewayConfigFile {
        GatewayConfigFile {
            heartbeat: heartbeat.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn valid_elevenlabs_tts_config_is_used_as_primary_tts() {
        let (_dir, env) = env_with_token();
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::write(
            &cfg_path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":30,"tts":{"provider":"elevenlabs","model":"eleven_v3","voice":"voice-abc","speed":1.25}}"#,
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(
            cfg.configured_tts().unwrap(),
            Some(ConfiguredTts::ElevenLabs {
                model: "eleven_v3".to_string(),
                voice: "voice-abc".to_string(),
                speed: Some(1.25),
            })
        );
        assert!(cfg.tts_fallback_warning().is_none());
    }

    #[test]
    fn missing_tts_config_uses_local_voicebox_without_warning() {
        let (_dir, env) = env_with_token();

        let cfg = load_from_env(&env).unwrap();

        assert_eq!(cfg.configured_tts().unwrap(), None);
        assert!(cfg.tts_fallback_warning().is_none());
    }

    #[test]
    fn invalid_tts_config_warns_and_falls_back_to_local_voicebox() {
        let (_dir, env) = env_with_token();
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::write(
            &cfg_path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":30,"tts":{"provider":17,"unknown":{"nested":true}}}"#,
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        let err = cfg.configured_tts().unwrap_err();
        assert!(err.contains("`tts.provider`"));
        assert!(err.contains("elevenlabs"));
        let warning = cfg.tts_fallback_warning().unwrap();
        assert!(warning.contains("Invalid `tts` config"));
        assert!(warning.contains("falling back to local Voicebox"));
    }

    #[test]
    fn elevenlabs_tts_config_requires_model_voice_and_positive_speed() {
        let (_dir, env) = env_with_token();
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::write(
            &cfg_path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":30,"tts":{"provider":"elevenlabs","model":" ","voice":"voice-abc","speed":0}}"#,
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        let err = cfg.configured_tts().unwrap_err();
        assert!(err.contains("`tts.model`"));
    }

    #[test]
    fn elevenlabs_tts_config_rejects_old_voice_id_field() {
        let (_dir, env) = env_with_token();
        let cfg_path =
            PathBuf::from(env.get("XDG_CONFIG_HOME").unwrap()).join("gateway/config.json");
        fs::write(
            &cfg_path,
            r#"{"models":[{"provider":"codex","model":"gpt-test"}],"timeout_mins":30,"tts":{"provider":"elevenlabs","model":"eleven_v3","voice_id":"voice-abc"}}"#,
        )
        .unwrap();

        let cfg = load_from_env(&env).unwrap();

        let err = cfg.configured_tts().unwrap_err();
        assert!(err.contains("unsupported `tts.voice_id` field"));
        assert!(err.contains("`voice`"));
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
        env.insert(
            GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(),
            "7,bad".to_string(),
        );
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("comma-separated integers"));

        let (_dir, mut env) = env_with_token();
        env.insert(GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(), " , ".to_string());
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("at least one id"));

        let (_dir, mut env) = env_with_token();
        env.insert(
            GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(),
            "42,-100".to_string(),
        );
        let err = load_from_env(&env).unwrap_err();
        assert!(err.contains("private chat ids"));
    }

    #[test]
    fn telegram_chat_ids_are_trimmed_sorted_and_deduplicated() {
        let (_dir, mut env) = env_with_token();
        env.insert(
            GATEWAY_TELEGRAM_CHAT_ID_ENV.to_string(),
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
