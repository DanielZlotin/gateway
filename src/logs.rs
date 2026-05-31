use crate::text::tail_log_plain_text;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub fn read_gateway_logs(env: &BTreeMap<String, String>, lines: usize) -> Result<String, String> {
    let log_file = gateway_log_file(env)?;
    Ok(fs::read_to_string(log_file)
        .map(|text| tail_log_plain_text(&text, lines))
        .unwrap_or_else(|_| "No gateway log available.".to_string()))
}

fn gateway_log_file(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    env.get("XDG_STATE_HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("gateway/logs/gateway.log"))
        .ok_or_else(|| "XDG_STATE_HOME is required".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_gateway_logs_uses_only_xdg_state_home_and_plain_line_breaks() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = dir.path().join("state/gateway/logs/gateway.log");
        std::fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        std::fs::write(&log_file, "one\ntwo\nthree\n").unwrap();
        let env = BTreeMap::from([(
            "XDG_STATE_HOME".to_string(),
            dir.path().join("state").to_string_lossy().to_string(),
        )]);

        let text = read_gateway_logs(&env, 2).unwrap();

        assert_eq!(text, "two\nthree");
    }

    #[test]
    fn read_gateway_logs_reports_missing_log_without_telegram_env() {
        let dir = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([(
            "XDG_STATE_HOME".to_string(),
            dir.path().join("state").to_string_lossy().to_string(),
        )]);

        let text = read_gateway_logs(&env, 10).unwrap();

        assert_eq!(text, "No gateway log available.");
    }

    #[test]
    fn read_gateway_logs_requires_xdg_state_home() {
        let err = read_gateway_logs(&BTreeMap::new(), 10).unwrap_err();
        assert_eq!(err, "XDG_STATE_HOME is required");
    }
}
