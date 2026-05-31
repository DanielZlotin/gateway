use crate::config::Config;
use crate::session::ChatSession;
use crate::text::session_label;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const CODEX_USAGE_TIMEOUT: Duration = Duration::from_secs(5);
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const FASTFETCH_TIMEOUT: Duration = Duration::from_secs(5);
const FASTFETCH_CONFIG: &str = r#"{
  "$schema": "https://github.com/fastfetch-cli/fastfetch/raw/dev/doc/json_schema.json",
  "modules": [
    "title",
    "separator",
    "os",
    "host",
    "kernel",
    "uptime",
    "packages",
    "shell",
    "display",
    "terminal",
    "cpu",
    "gpu",
    "memory",
    "disk",
    "battery",
    "poweradapter",
    "locale",
    "editor",
    "datetime",
    {
      "key": "Timezone",
      "type": "command",
      "text": "date +%Z",
      "shell": "zsh"
    },
    {
      "key": "Day of Week",
      "type": "command",
      "text": "date +%A",
      "shell": "zsh"
    },
    {
      "key": "Weather",
      "type": "weather",
      "timeout": 3000
    }
  ]
}
"#;

pub fn status_header(state: &ChatSession) -> String {
    format!(
        "Model: {}\nSession: {}",
        state.model,
        session_label(state.session_id.as_deref().unwrap_or(""))
    )
}

pub fn format_status_message(state: &ChatSession, codex: &str, fetch: &str) -> String {
    let mut sections = vec![status_header(state)];
    for section in [codex, fetch] {
        let section = section.trim();
        if !section.is_empty() {
            sections.push(section.to_string());
        }
    }
    sections.join("\n\n")
}

pub fn codex_status(cfg: &Config) -> String {
    match read_codex_backend_auth(&cfg.xdg_config_home.join("codex"))
        .and_then(|auth| fetch_codex_usage_backend(&auth, CODEX_USAGE_URL, CODEX_USAGE_TIMEOUT))
    {
        Ok(usage) => format_codex_usage(&usage),
        Err(err) => format!("Codex: {err}"),
    }
}

pub fn fastfetch_status(bin: &Path) -> String {
    match run_fastfetch(bin, FASTFETCH_TIMEOUT) {
        Ok((raw, timed_out)) => format_fastfetch_status(&raw, timed_out),
        Err(err) => format!("fastfetch: {err}"),
    }
}

fn format_fastfetch_status(raw: &str, timed_out: bool) -> String {
    let text = format_fastfetch_output(raw);
    match (text.is_empty(), timed_out) {
        (false, false) => text,
        (false, true) => format!("{text}\n• ⏳ Fastfetch: timed out; showing partial output"),
        (true, true) => "fastfetch: timed out".to_string(),
        (true, false) => "fastfetch: no output".to_string(),
    }
}

fn run_fastfetch(bin: &Path, timeout: Duration) -> Result<(String, bool), String> {
    let mut child = Command::new(bin)
        .args(["--config", "-", "--pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    let mut config_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "open config pipe".to_string())?;
    config_stdin
        .write_all(FASTFETCH_CONFIG.as_bytes())
        .map_err(|err| format!("write config: {err}"))?;
    drop(config_stdin);
    let (output, timed_out) = wait_with_timeout(child, timeout)?;
    if timed_out || output.status.success() {
        return Ok((
            String::from_utf8_lossy(&output.stdout).to_string(),
            timed_out,
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!("exited with {} {stderr}", output.status))
}

fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<(Output, bool), String> {
    let start = Instant::now();
    loop {
        if child.try_wait().map_err(|err| err.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            return Ok((output, false));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            return Ok((output, true));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn format_fastfetch_output(raw: &str) -> String {
    let mut lines = Vec::new();
    for raw_line in strip_ansi(raw).lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('-') || !line.contains(':') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .expect("line contains a colon after earlier check");
        let key = key.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let Some(emoji) = fastfetch_emoji(key) else {
            continue;
        };
        lines.push(format!("• {emoji} {key}: {value}"));
    }
    lines.join("\n")
}

#[cfg(test)]
fn format_codex_usage_json(raw: &str) -> String {
    serde_json::from_str::<CodexUsageResponse>(raw).map_or_else(
        |_| "Codex: usage response unreadable".to_string(),
        |usage| format_codex_usage(&usage),
    )
}

fn format_codex_usage(usage: &CodexUsageResponse) -> String {
    let Some(rate_limit) = usage.rate_limit.as_ref() else {
        return "Codex: usage unavailable".to_string();
    };
    let mut parts = Vec::new();
    if let Some(window) = rate_limit.primary_window.as_ref() {
        if let Some(part) = format_codex_usage_window(primary_usage_label(window), window) {
            parts.push(part);
        }
    }
    if let Some(window) = rate_limit.secondary_window.as_ref() {
        if let Some(part) = format_codex_usage_window(
            secondary_usage_label(window, rate_limit.primary_window.as_ref()),
            window,
        ) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        "Codex: usage unavailable".to_string()
    } else {
        format!("Codex: {}", parts.join(" · "))
    }
}

fn format_codex_usage_window(label: String, window: &CodexUsageWindow) -> Option<String> {
    let used_percent = window.used_percent?;
    let remaining = (100.0 - used_percent).clamp(0.0, 100.0).round();
    Some(format!("{label} {remaining:.0}% left"))
}

fn read_codex_backend_auth(auth_dir: &Path) -> Result<CodexBackendAuth, String> {
    let auth_path = auth_dir.join("auth.json");
    let raw = fs::read_to_string(&auth_path).map_err(|err| format!("read auth: {err}"))?;
    let auth: Value = serde_json::from_str(&raw).map_err(|err| format!("parse auth: {err}"))?;
    let access_token = json_string_at(&auth, &["tokens", "access_token"])
        .or_else(|| json_string_at(&auth, &["tokens", "accessToken"]))
        .or_else(|| json_string_at(&auth, &["access_token"]))
        .or_else(|| json_string_at(&auth, &["accessToken"]))
        .ok_or_else(|| "auth missing access token".to_string())?
        .to_string();
    let account_id = json_string_at(&auth, &["tokens", "account_id"])
        .or_else(|| json_string_at(&auth, &["tokens", "accountId"]))
        .or_else(|| json_string_at(&auth, &["tokens", "id_token", "chatgpt_account_id"]))
        .or_else(|| json_string_at(&auth, &["tokens", "id_token", "account_id"]))
        .or_else(|| {
            json_string_at(
                &auth,
                &[
                    "tokens",
                    "id_token",
                    "https://api.openai.com/auth",
                    "chatgpt_account_id",
                ],
            )
        })
        .or_else(|| json_string_at(&auth, &["account_id"]))
        .or_else(|| json_string_at(&auth, &["accountId"]))
        .map(str::to_string);

    Ok(CodexBackendAuth {
        access_token,
        account_id,
    })
}

fn fetch_codex_usage_backend(
    auth: &CodexBackendAuth,
    url: &str,
    timeout: Duration,
) -> Result<CodexUsageResponse, String> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let authorization = format!("Bearer {}", auth.access_token);
    let mut request = agent
        .get(url)
        .set("Authorization", &authorization)
        .set("Accept", "application/json")
        .set("originator", "gateway")
        .set("User-Agent", "gateway");
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.set("ChatGPT-Account-Id", account_id);
    }
    let response = request.call().map_err(format_codex_usage_request_error)?;
    response
        .into_json::<CodexUsageResponse>()
        .map_err(|err| format!("parse usage: {err}"))
}

fn format_codex_usage_request_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, _) => format!("usage request failed: HTTP {code}"),
        ureq::Error::Transport(err) => format!("usage request failed: {err}"),
    }
}

fn json_string_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn primary_usage_label(window: &CodexUsageWindow) -> String {
    window
        .limit_window_seconds
        .and_then(duration_usage_label)
        .unwrap_or_else(|| "usage".to_string())
}

fn secondary_usage_label(
    window: &CodexUsageWindow,
    primary_window: Option<&CodexUsageWindow>,
) -> String {
    if secondary_reset_is_far(window, primary_window) {
        return "weekly".to_string();
    }
    window
        .limit_window_seconds
        .and_then(duration_usage_label)
        .unwrap_or_else(|| "weekly".to_string())
}

fn secondary_reset_is_far(
    window: &CodexUsageWindow,
    primary_window: Option<&CodexUsageWindow>,
) -> bool {
    let Some(primary_window) = primary_window else {
        return false;
    };
    let Some(primary_reset) = reset_value(primary_window) else {
        return false;
    };
    let Some(secondary_reset) = reset_value(window) else {
        return false;
    };
    (secondary_reset - primary_reset).abs() >= 3.0 * 24.0 * 60.0 * 60.0
}

fn reset_value(window: &CodexUsageWindow) -> Option<f64> {
    window
        .reset_at
        .or(window.reset_after_seconds)
        .filter(|value| value.is_finite())
}

fn duration_usage_label(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let seconds = seconds.round() as u64;
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 28 * DAY;

    if seconds >= MONTH {
        Some("monthly".to_string())
    } else if seconds >= WEEK {
        Some("weekly".to_string())
    } else if seconds == DAY {
        Some("daily".to_string())
    } else if seconds > DAY && seconds.is_multiple_of(DAY) {
        Some(format!("{}d", seconds / DAY))
    } else if seconds.is_multiple_of(HOUR) {
        Some(format!("{}h", seconds / HOUR))
    } else if seconds.is_multiple_of(MINUTE) {
        Some(format!("{}m", seconds / MINUTE))
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexBackendAuth {
    access_token: String,
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageResponse {
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexUsageWindow>,
    secondary_window: Option<CodexUsageWindow>,
}

#[derive(Debug, Deserialize)]
struct CodexUsageWindow {
    limit_window_seconds: Option<f64>,
    used_percent: Option<f64>,
    reset_at: Option<f64>,
    reset_after_seconds: Option<f64>,
}

fn fastfetch_emoji(key: &str) -> Option<&'static str> {
    match key {
        "OS" => Some("🖥️"),
        "Host" => Some("💻"),
        "Kernel" => Some("⚙️"),
        "Uptime" => Some("⏱️"),
        "Packages" => Some("📦"),
        "Shell" => Some("🐚"),
        key if key.starts_with("Display") => Some("🖼️"),
        "Terminal" => Some("🖥️"),
        "CPU" => Some("🧠"),
        "GPU" => Some("🎮"),
        "Memory" => Some("💾"),
        "Swap" => Some("🔁"),
        key if key.starts_with("Disk") => Some("🗄️"),
        key if key.starts_with("Local IP") => Some("🌐"),
        key if key.starts_with("Battery") => Some("🔋"),
        "Power Adapter" => Some("🔌"),
        "Locale" => Some("🌍"),
        "Bluetooth" => Some("🟦"),
        "Editor" => Some("✏️"),
        "Date & Time" | "DateTime" | "Datetime" => Some("📅"),
        "Hebrew Date" => Some("✡️"),
        "Timezone" => Some("🕒"),
        "Day of Week" => Some("📆"),
        "Moon Phase" => Some("🌙"),
        "Weather" => Some("☁️"),
        _ => None,
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        for next in chars.by_ref() {
            if ('@'..='~').contains(&next) {
                break;
            }
        }
    }
    out
}

pub const fn typing_interval() -> Duration {
    Duration::from_secs(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{BufRead, BufReader, Write as IoWrite};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;

    #[test]
    fn status_header_does_not_include_command_help() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = status_header(&state);

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("Session: 12345678"));
        assert!(!got.contains("/commands"));
    }

    #[test]
    fn format_status_message_appends_codex_before_fetch() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = format_status_message(&state, "Codex: ok", "OS: test");

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("Codex: ok\n\nOS: test"));
        assert!(got.contains("OS: test"));
        assert!(!got.contains("Gateway restarted."));
    }

    #[test]
    fn codex_usage_json_formats_backendapi_limit_windows() {
        let raw = r#"{
          "rate_limit": {
            "primary_window": {
              "limit_window_seconds": 18000,
              "used_percent": 12.4
            },
            "secondary_window": {
              "limit_window_seconds": 604800,
              "used_percent": 1.2
            }
          }
        }"#;

        let got = format_codex_usage_json(raw);

        assert_eq!(got, "Codex: 5h 88% left · weekly 99% left");
    }

    #[test]
    fn codex_usage_json_labels_far_daily_reset_as_weekly() {
        let raw = r#"{
          "rate_limit": {
            "primary_window": {
              "limit_window_seconds": 18000,
              "used_percent": 20,
              "reset_at": 2000000000
            },
            "secondary_window": {
              "limit_window_seconds": 86400,
              "used_percent": 75,
              "reset_at": 2000432000
            }
          }
        }"#;

        let got = format_codex_usage_json(raw);

        assert_eq!(got, "Codex: 5h 80% left · weekly 25% left");
    }

    #[test]
    fn codex_usage_json_reports_unavailable_when_backendapi_has_no_windows() {
        assert_eq!(format_codex_usage_json("{}"), "Codex: usage unavailable");
        assert_eq!(
            format_codex_usage_json("not json"),
            "Codex: usage response unreadable"
        );
    }

    #[test]
    fn fastfetch_output_formats_telegram_bullets() {
        let raw = "\x1b[34C------------------------------\n\x1b[34COS: macOS Tahoe 26.3\n\x1b[34CHost: MacBook Pro\n\x1b[34CPackages: 126\n\x1b[34CMemory: 8.60 GiB / 64.00 GiB (13%)\n";

        let got = format_fastfetch_output(raw);

        assert_eq!(
            got,
            "• 🖥️ OS: macOS Tahoe 26.3\n• 💻 Host: MacBook Pro\n• 📦 Packages: 126\n• 💾 Memory: 8.60 GiB / 64.00 GiB (13%)"
        );
    }

    #[test]
    fn fastfetch_status_formats_timeout_states() {
        assert_eq!(format_fastfetch_status("", true), "fastfetch: timed out");
        assert_eq!(format_fastfetch_status("", false), "fastfetch: no output");
        assert_eq!(
            format_fastfetch_status("OS: macOS\n", true),
            "• 🖥️ OS: macOS\n• ⏳ Fastfetch: timed out; showing partial output"
        );
    }

    #[test]
    fn fastfetch_receives_embedded_config_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let fake_fastfetch = executable(
            dir.path().join("fastfetch"),
            r#"#!/bin/sh
printf 'ARGS:'
for arg in "$@"; do
  printf '[%s]' "$arg"
done
if [ "$1" = "--config" ] && [ "$2" = "-" ]; then
  printf '\nSTDIN:\n'
  cat
fi
"#,
        );

        let (raw, timed_out) = run_fastfetch(&fake_fastfetch, Duration::from_secs(5)).unwrap();

        assert!(!timed_out);
        assert!(raw.contains("ARGS:[--config][-][--pipe]\n"));
        assert!(raw.contains(
            r#""$schema": "https://github.com/fastfetch-cli/fastfetch/raw/dev/doc/json_schema.json""#
        ));
        assert!(raw.contains(r#""modules": ["#));
        assert!(raw.contains(r#""title""#));
        assert!(raw.contains(r#""key": "Timezone""#));
        assert!(raw.contains(r#""type": "weather""#));
    }

    #[test]
    fn fastfetch_timeout_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let fake_fastfetch = executable(
            dir.path().join("fastfetch"),
            r#"#!/bin/sh
cat >/dev/null
/bin/echo 'OS: partial'
exec sleep 1
"#,
        );

        let (raw, timed_out) = run_fastfetch(&fake_fastfetch, Duration::from_millis(250)).unwrap();

        assert!(timed_out);
        assert!(raw.is_empty() || raw.contains("OS: partial"));
    }

    #[test]
    fn fastfetch_nonzero_exit_reports_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let fake_fastfetch = executable(
            dir.path().join("fastfetch"),
            r#"#!/bin/sh
printf 'bad config\n' >&2
exit 2
"#,
        );

        let err = run_fastfetch(&fake_fastfetch, Duration::from_secs(5)).unwrap_err();

        assert!(err.contains("exited with"));
        assert!(err.contains("bad config"));
    }

    #[test]
    fn codex_auth_reads_backendapi_token_and_account() {
        let dir = tempfile::tempdir().unwrap();
        let auth_dir = dir.path().join("codex");
        fs::create_dir(&auth_dir).unwrap();
        fs::write(
            auth_dir.join("auth.json"),
            r#"{
              "auth_mode": "chatgpt",
              "tokens": {
                "id_token": {
                  "chatgpt_account_id": "acc_from_id_token"
                },
                "access_token": "access_token_value",
                "refresh_token": "refresh_token_value",
                "account_id": "acc_from_token"
              }
            }"#,
        )
        .unwrap();

        let auth = read_codex_backend_auth(&auth_dir).unwrap();

        assert_eq!(auth.access_token, "access_token_value");
        assert_eq!(auth.account_id.as_deref(), Some("acc_from_token"));
    }

    #[test]
    fn codex_usage_fetch_sends_backendapi_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/backend-api/wham/usage",
            listener.local_addr().unwrap()
        );
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            loop {
                let mut line = String::new();
                let read = reader.read_line(&mut line).unwrap();
                if read == 0 || line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            let body = r#"{"rate_limit":{"primary_window":{"limit_window_seconds":18000,"used_percent":40},"secondary_window":{"limit_window_seconds":604800,"used_percent":5}}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        let auth = CodexBackendAuth {
            access_token: "token_value".to_string(),
            account_id: Some("acc_test".to_string()),
        };

        let usage = fetch_codex_usage_backend(&auth, &url, Duration::from_secs(5)).unwrap();
        let request = server.join().unwrap().to_ascii_lowercase();

        assert_eq!(
            format_codex_usage(&usage),
            "Codex: 5h 60% left · weekly 95% left"
        );
        assert!(request.starts_with("get /backend-api/wham/usage http/1.1\r\n"));
        assert!(request.contains("authorization: bearer token_value\r\n"));
        assert!(request.contains("accept: application/json\r\n"));
        assert!(request.contains("originator: gateway\r\n"));
        assert!(!request.contains("version:"));
        assert!(request.contains("user-agent: gateway\r\n"));
        assert!(request.contains("chatgpt-account-id: acc_test\r\n"));
    }

    #[test]
    fn fastfetch_status_reports_start_errors_and_known_keys() {
        let missing = fastfetch_status(Path::new("/definitely/missing/fastfetch"));
        assert!(missing.contains("fastfetch:"));

        let raw = [
            "Kernel: Darwin",
            "Uptime: 1 day",
            "Shell: zsh",
            "Display 1: 3024x1964",
            "Terminal: ghostty",
            "CPU: M4",
            "GPU: Apple",
            "Swap: 0 B",
            "Disk (/): 1 TiB",
            "Local IP (en0): 127.0.0.1",
            "Battery: 100%",
            "Power Adapter: Connected",
            "Locale: en_US",
            "Bluetooth: On",
            "Editor: nvim",
            "DateTime: today",
            "Hebrew Date: 1 Nisan",
            "Timezone: UTC",
            "Day of Week: Sunday",
            "Moon Phase: New",
            "Weather: Clear",
            "Ignored: value",
            "No value:",
            "no separator",
            "\x1bXnot-csi",
        ]
        .join("\n");
        let formatted = format_fastfetch_output(&raw);

        assert!(formatted.contains("• ⚙️ Kernel: Darwin"));
        assert!(formatted.contains("• 🖼️ Display 1: 3024x1964"));
        assert!(formatted.contains("• 🌐 Local IP (en0): 127.0.0.1"));
        assert!(formatted.contains("• 🌙 Moon Phase: New"));
        assert!(!formatted.contains("Ignored"));
        assert_eq!(typing_interval(), Duration::from_secs(4));
    }

    fn executable(path: std::path::PathBuf, body: &str) -> std::path::PathBuf {
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
}
