use crate::text::tail_log_plain_text;
use std::collections::BTreeMap;
use std::fmt::Arguments;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn read_gateway_logs(env: &BTreeMap<String, String>, lines: usize) -> Result<String, String> {
    let log_file = gateway_log_file(env)?;
    Ok(fs::read_to_string(log_file)
        .map(|text| tail_log_plain_text(&text, lines))
        .unwrap_or_else(|_| "No gateway log available.".to_string()))
}

fn gateway_log_file(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    crate::config::resolve_xdg_state_home(env).map(|path| path.join("gateway/logs/gateway.log"))
}

pub fn info(args: Arguments<'_>) {
    write("INFO", args);
}

pub fn warn(args: Arguments<'_>) {
    write("WARN", args);
}

pub fn error(args: Arguments<'_>) {
    write("ERROR", args);
}

fn write(level: &str, args: Arguments<'_>) {
    eprintln!(
        "{}",
        format_log_line(
            &current_utc_timestamp(),
            env!("CARGO_PKG_VERSION"),
            level,
            &args.to_string()
        )
    );
}

pub fn format_log_line(timestamp: &str, version: &str, level: &str, message: &str) -> String {
    let level = level.trim();
    format!(
        "{} {timestamp} v={version} {}",
        log_icon(level),
        one_line(message)
    )
}

fn log_icon(level: &str) -> &'static str {
    match level {
        "INFO" => "ℹ️",
        "WARN" => "⚠️",
        "ERROR" => "❌",
        _ => "🧾",
    }
}

pub fn current_utc_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_utc_timestamp(seconds)
}

fn format_utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let (year, month, day) = civil_from_unix_days(days);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

fn civil_from_unix_days(days: i64) -> (i64, i64, i64) {
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = month_index + if month_index < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

fn one_line(message: &str) -> String {
    message.lines().collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_log_line_includes_timestamp_version_level_icon_and_message() {
        let line = format_log_line("2026-06-02 15:30:43", "0.1.6", "WARN", "poll failed");

        assert_eq!(line, "⚠️ 2026-06-02 15:30:43 v=0.1.6 poll failed");
    }

    #[test]
    fn format_log_line_flattens_multiline_messages() {
        let line = format_log_line("2026-06-02 15:30:43", "0.1.6", " WARN ", "one\ntwo");

        assert_eq!(line, "⚠️ 2026-06-02 15:30:43 v=0.1.6 one | two");
    }

    #[test]
    fn format_log_line_uses_level_icons() {
        assert_eq!(
            format_log_line("2026-06-02 15:30:43", "0.1.6", "INFO", "started"),
            "ℹ️ 2026-06-02 15:30:43 v=0.1.6 started"
        );
        assert_eq!(
            format_log_line("2026-06-02 15:30:43", "0.1.6", "ERROR", "failed"),
            "❌ 2026-06-02 15:30:43 v=0.1.6 failed"
        );
    }

    #[test]
    fn format_utc_timestamp_handles_epoch_and_leap_days() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc_timestamp(951_782_400), "2000-02-29 00:00:00");
    }

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
    fn read_gateway_logs_uses_default_xdg_state_home_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let log_file = home.join(".local/state/gateway/logs/gateway.log");
        std::fs::create_dir_all(log_file.parent().unwrap()).unwrap();
        std::fs::write(&log_file, "one\ntwo\n").unwrap();
        let env = BTreeMap::from([("HOME".to_string(), home.to_string_lossy().to_string())]);

        let text = read_gateway_logs(&env, 1).unwrap();

        assert_eq!(text, "two");
    }

    #[test]
    fn read_gateway_logs_requires_home_when_xdg_state_home_is_unset() {
        let err = read_gateway_logs(&BTreeMap::new(), 10).unwrap_err();
        assert_eq!(err, "HOME is required");
    }
}
