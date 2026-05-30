# Rust Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a lean Rust rewrite of the Go Telegram-to-Codex gateway with bot chat mode and external cron/launchd `run` mode.

**Architecture:** The binary has two modes: `gateway bot` for Telegram long polling and `gateway run` for one-shot scheduled Codex prompts. Core behavior is split into small modules with pure functions for parsing, formatting, config, command decisions, session mutation, Codex args, and Telegram request construction; I/O modules stay thin.

**Tech Stack:** Rust 2021, `clap`, `serde`, `serde_json`, `ureq`, `tempfile`, standard-library threads/channels/process/filesystem.

---

## File Structure

- Create `Cargo.toml`: crate metadata, dependencies, lint profile.
- Create `src/lib.rs`: module exports for tests and binary wiring.
- Create `src/main.rs`: CLI entrypoint and process exit mapping.
- Create `src/cli.rs`: `gateway bot` and `gateway run` argument parsing plus prompt source precedence.
- Create `src/config.rs`: defaults, environment parsing, runtime paths, allowlist.
- Create `src/text.rs`: Telegram splitting, command argument extraction, log tailing, joining.
- Create `src/session.rs`: JSON state, chat keys, cron keys, mutation methods.
- Create `src/codex.rs`: Codex command args/env, JSON output parsing, process runner.
- Create `src/telegram.rs`: DTOs, request encoding, command scopes, HTTP client.
- Create `src/commands.rs`: Telegram command parser and command response decisions.
- Create `src/status.rs`: status text and `fastfetch` wrapper.
- Create `src/bot.rs`: poll loop, offset persistence, queue worker, job execution.
- Create `src/run_mode.rs`: one-shot cron/launchd execution.
- Create `ai.gateway.plist`: macOS LaunchAgent equivalent of the Go gateway.
- Create `gateway-launch.sh`: zsh launcher for the bot process.
- Create `README.md`: build, install, bot, and cron examples.

## Task 1: Scaffold Rust Crate

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/cli.rs`

- [ ] **Step 1: Write failing CLI tests**

Create `src/cli.rs` with these tests and minimal type declarations above them so the file compiles far enough to fail on missing behavior:

```rust
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Bot,
    Run(RunArgs),
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub job: String,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    pub new_session: bool,
    pub telegram_chat: Option<i64>,
}

pub fn parse_args_from<I, T>(_args: I) -> Result<Mode, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Err("missing behavior".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_bot_mode_when_no_subcommand_is_given() {
        let mode = parse_args_from(["gateway"]).unwrap();
        assert_eq!(mode, Mode::Bot);
    }

    #[test]
    fn parses_run_mode_with_prompt_and_delivery_chat() {
        let mode = parse_args_from([
            "gateway",
            "run",
            "--job",
            "daily",
            "--prompt",
            "summarize",
            "--model",
            "gpt-test",
            "--new",
            "--telegram-chat",
            "42",
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Run(RunArgs {
                job: "daily".to_string(),
                prompt: Some("summarize".to_string()),
                prompt_file: None,
                model: Some("gpt-test".to_string()),
                new_session: true,
                telegram_chat: Some(42),
            })
        );
    }

    #[test]
    fn run_mode_requires_job() {
        let err = parse_args_from(["gateway", "run", "--prompt", "hello"]).unwrap_err();
        assert!(err.contains("--job"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test cli::tests -- --nocapture`

Expected: the command fails because `Cargo.toml`, `src/lib.rs`, and `src/main.rs` do not exist yet.

- [ ] **Step 3: Create crate files and CLI implementation**

Create `Cargo.toml`:

```toml
[package]
name = "gateway"
version = "0.1.0"
edition = "2021"
license = "MIT"

[dependencies]
clap = { version = "4.5.38", features = ["derive"] }
serde = { version = "1.0.203", features = ["derive"] }
serde_json = "1.0.117"
tempfile = "3.10.1"
ureq = { version = "2.10.1", features = ["json"] }

[profile.release]
strip = true
lto = true
codegen-units = 1
```

Create `src/lib.rs`:

```rust
pub mod cli;
```

Create `src/main.rs`:

```rust
use gateway::cli::{parse_args_from, Mode};

fn main() {
    match parse_args_from(std::env::args_os()) {
        Ok(Mode::Bot) => {
            eprintln!("gateway bot is not wired yet");
            std::process::exit(2);
        }
        Ok(Mode::Run(_args)) => {
            eprintln!("gateway run is not wired yet");
            std::process::exit(2);
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}
```

Replace `src/cli.rs` with:

```rust
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Bot,
    Run(RunArgs),
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub job: String,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    pub new_session: bool,
    pub telegram_chat: Option<i64>,
}

#[derive(Debug, Parser)]
#[command(name = "gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Bot,
    Run(RunCli),
}

#[derive(Debug, Args)]
struct RunCli {
    #[arg(long)]
    job: String,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    new: bool,
    #[arg(long)]
    telegram_chat: Option<i64>,
}

pub fn parse_args_from<I, T>(args: I) -> Result<Mode, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|err| err.to_string())?;
    Ok(match cli.command {
        None | Some(Command::Bot) => Mode::Bot,
        Some(Command::Run(args)) => Mode::Run(RunArgs {
            job: args.job,
            prompt: args.prompt,
            prompt_file: args.prompt_file,
            model: args.model,
            new_session: args.new,
            telegram_chat: args.telegram_chat,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_bot_mode_when_no_subcommand_is_given() {
        let mode = parse_args_from(["gateway"]).unwrap();
        assert_eq!(mode, Mode::Bot);
    }

    #[test]
    fn parses_run_mode_with_prompt_and_delivery_chat() {
        let mode = parse_args_from([
            "gateway",
            "run",
            "--job",
            "daily",
            "--prompt",
            "summarize",
            "--model",
            "gpt-test",
            "--new",
            "--telegram-chat",
            "42",
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Run(RunArgs {
                job: "daily".to_string(),
                prompt: Some("summarize".to_string()),
                prompt_file: None,
                model: Some("gpt-test".to_string()),
                new_session: true,
                telegram_chat: Some(42),
            })
        );
    }

    #[test]
    fn run_mode_requires_job() {
        let err = parse_args_from(["gateway", "run", "--prompt", "hello"]).unwrap_err();
        assert!(err.contains("--job"));
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test cli::tests -- --nocapture`

Expected: all three CLI tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/main.rs src/cli.rs
git commit -m "feat: scaffold rust gateway cli"
```

## Task 2: Text Utilities

**Files:**
- Create: `src/text.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing text utility tests**

Create `src/text.rs`:

```rust
pub const TELEGRAM_MESSAGE_LIMIT: usize = 3900;

pub fn split_telegram_message(_text: &str) -> Vec<String> {
    Vec::new()
}

pub fn parse_command(_text: &str) -> Option<String> {
    None
}

pub fn command_arg(_text: &str) -> String {
    String::new()
}

pub fn session_label(_session_id: &str) -> String {
    String::new()
}

pub fn log_line_count(_text: &str) -> usize {
    0
}

pub fn tail_log_text(_text: &str, _lines: usize) -> String {
    String::new()
}

pub fn join_non_empty(parts: &[&str]) -> String {
    parts.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_telegram_message_keeps_parts_under_limit() {
        let text = "a".repeat(TELEGRAM_MESSAGE_LIMIT + 50);
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|part| part.chars().count() <= TELEGRAM_MESSAGE_LIMIT));
    }

    #[test]
    fn split_telegram_message_prefers_recent_newline() {
        let text = format!("{}\\n{}", "a".repeat(TELEGRAM_MESSAGE_LIMIT - 20), "b".repeat(100));
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].chars().last(), Some('a'));
        assert!(parts[1].starts_with('b'));
    }

    #[test]
    fn parse_command_strips_bot_suffix_and_lowercases() {
        assert_eq!(parse_command("/Log@MyBot 20"), Some("/log".to_string()));
        assert_eq!(parse_command("normal prompt"), None);
    }

    #[test]
    fn command_arg_preserves_spaces_after_command() {
        assert_eq!(command_arg("/rename work session"), "work session");
        assert_eq!(command_arg("/list"), "");
    }

    #[test]
    fn session_label_uses_short_id_or_none() {
        assert_eq!(session_label(""), "none");
        assert_eq!(session_label("12345678"), "12345678");
        assert_eq!(session_label("123456789abc"), "12345678");
    }

    #[test]
    fn log_line_count_defaults_and_caps() {
        assert_eq!(log_line_count("/log"), 80);
        assert_eq!(log_line_count("/log 10"), 10);
        assert_eq!(log_line_count("/log bad"), 80);
        assert_eq!(log_line_count("/log 0"), 80);
        assert_eq!(log_line_count("/log 999"), 200);
    }

    #[test]
    fn tail_log_text_returns_last_lines() {
        assert_eq!(tail_log_text("one\\ntwo\\nthree\\n", 2), "two\\nthree");
        assert_eq!(tail_log_text("", 2), "Gateway log is empty.");
    }

    #[test]
    fn join_non_empty_trims_and_separates() {
        assert_eq!(join_non_empty(&[" hello ", "", " world "]), "hello\\n\\nworld");
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test text::tests -- --nocapture`

Expected: tests fail because the functions return empty defaults.

- [ ] **Step 3: Implement text utilities**

Replace `src/text.rs` with:

```rust
pub const TELEGRAM_MESSAGE_LIMIT: usize = 3900;

pub fn split_telegram_message(text: &str) -> Vec<String> {
    let mut rest = text.trim().to_string();
    if rest.is_empty() {
        return vec![String::new()];
    }

    let mut parts = Vec::new();
    while rest.chars().count() > TELEGRAM_MESSAGE_LIMIT {
        let chars: Vec<char> = rest.chars().collect();
        let mut split_at = TELEGRAM_MESSAGE_LIMIT;
        let floor = TELEGRAM_MESSAGE_LIMIT.saturating_sub(600);

        for index in (floor..TELEGRAM_MESSAGE_LIMIT).rev() {
            if chars[index] == '\n' {
                split_at = index + 1;
                break;
            }
        }

        parts.push(chars[..split_at].iter().collect::<String>().trim().to_string());
        rest = chars[split_at..].iter().collect::<String>().trim().to_string();
    }

    parts.push(rest);
    parts
}

pub fn parse_command(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?;
    if !first.starts_with('/') {
        return None;
    }
    let command = first
        .split('@')
        .next()
        .unwrap_or(first)
        .to_ascii_lowercase();
    Some(command)
}

pub fn command_arg(text: &str) -> String {
    let mut parts = text.splitn(2, char::is_whitespace);
    let _command = parts.next();
    parts.next().unwrap_or("").trim().to_string()
}

pub fn session_label(session_id: &str) -> String {
    if session_id.is_empty() {
        return "none".to_string();
    }
    session_id.chars().take(8).collect()
}

pub fn log_line_count(text: &str) -> usize {
    text.split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(200))
        .unwrap_or(80)
}

pub fn tail_log_text(text: &str, lines: usize) -> String {
    let text = text.trim();
    if text.is_empty() {
        return "Gateway log is empty.".to_string();
    }
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

pub fn join_non_empty(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_telegram_message_keeps_parts_under_limit() {
        let text = "a".repeat(TELEGRAM_MESSAGE_LIMIT + 50);
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|part| part.chars().count() <= TELEGRAM_MESSAGE_LIMIT));
    }

    #[test]
    fn split_telegram_message_prefers_recent_newline() {
        let text = format!("{}\\n{}", "a".repeat(TELEGRAM_MESSAGE_LIMIT - 20), "b".repeat(100));
        let parts = split_telegram_message(&text);

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].chars().last(), Some('a'));
        assert!(parts[1].starts_with('b'));
    }

    #[test]
    fn parse_command_strips_bot_suffix_and_lowercases() {
        assert_eq!(parse_command("/Log@MyBot 20"), Some("/log".to_string()));
        assert_eq!(parse_command("normal prompt"), None);
    }

    #[test]
    fn command_arg_preserves_spaces_after_command() {
        assert_eq!(command_arg("/rename work session"), "work session");
        assert_eq!(command_arg("/list"), "");
    }

    #[test]
    fn session_label_uses_short_id_or_none() {
        assert_eq!(session_label(""), "none");
        assert_eq!(session_label("12345678"), "12345678");
        assert_eq!(session_label("123456789abc"), "12345678");
    }

    #[test]
    fn log_line_count_defaults_and_caps() {
        assert_eq!(log_line_count("/log"), 80);
        assert_eq!(log_line_count("/log 10"), 10);
        assert_eq!(log_line_count("/log bad"), 80);
        assert_eq!(log_line_count("/log 0"), 80);
        assert_eq!(log_line_count("/log 999"), 200);
    }

    #[test]
    fn tail_log_text_returns_last_lines() {
        assert_eq!(tail_log_text("one\\ntwo\\nthree\\n", 2), "two\\nthree");
        assert_eq!(tail_log_text("", 2), "Gateway log is empty.");
    }

    #[test]
    fn join_non_empty_trims_and_separates() {
        assert_eq!(join_non_empty(&[" hello ", "", " world "]), "hello\\n\\nworld");
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test text::tests -- --nocapture`

Expected: all eight text tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/text.rs
git commit -m "feat: add text helpers"
```

## Task 3: Configuration

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing config tests**

Create `src/config.rs`:

```rust
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

pub fn load_from_env(_env: &BTreeMap<String, String>) -> Result<Config, String> {
    Err("missing behavior".to_string())
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
        assert_eq!(cfg.codex_home, PathBuf::from("/Users/example/.config/codex"));
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
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test config::tests -- --nocapture`

Expected: config tests fail because `load_from_env` returns `Err("missing behavior")`.

- [ ] **Step 3: Implement config loading**

Replace `src/config.rs` with:

```rust
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
    let state_dir = path(env, "GATEWAY_STATE_DIR", "/Users/example/.local/state/gateway");
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

fn path(env: &BTreeMap<String, String>, key: &str, default: &str) -> PathBuf {
    PathBuf::from(value(env, key, default))
}

fn number<T>(env: &BTreeMap<String, String>, key: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr + Copy,
{
    match env.get(key).map(|value| value.trim()).filter(|value| !value.is_empty()) {
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
        ids.push(
            part.parse::<i64>()
                .map_err(|_| "GATEWAY_ALLOWED_IDS must contain comma-separated integers".to_string())?,
        );
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
        assert_eq!(cfg.codex_home, PathBuf::from("/Users/example/.config/codex"));
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test config::tests -- --nocapture`

Expected: all three config tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/config.rs
git commit -m "feat: add runtime config"
```

## Task 4: Session Store

**Files:**
- Create: `src/session.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing session tests**

Create `src/session.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    pub session_id: Option<String>,
    pub model: String,
    pub generation: i64,
    pub updated_at: String,
    pub sessions: Vec<SavedSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    pub name: Option<String>,
    pub model: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub enum SessionKey {
    Chat(i64),
    Cron(String),
}

pub struct SessionStore {
    chat_dir: PathBuf,
    cron_dir: PathBuf,
    default_model: String,
}

impl SessionStore {
    pub fn new(chat_dir: PathBuf, cron_dir: PathBuf, default_model: String) -> Self {
        Self { chat_dir, cron_dir, default_model }
    }

    pub fn load(&self, _key: &SessionKey) -> ChatSession {
        ChatSession::default()
    }

    pub fn reset(&self, _key: &SessionKey) -> Result<ChatSession, String> {
        Err("missing behavior".to_string())
    }

    pub fn save_run(&self, _key: &SessionKey, _expected_generation: i64, _session_id: &str) -> Result<bool, String> {
        Err("missing behavior".to_string())
    }
}

pub fn upsert_session(items: Vec<SavedSession>, _item: SavedSession, _default_model: &str) -> Vec<SavedSession> {
    items
}

pub fn find_session(_items: &[SavedSession], _target: &str) -> Option<SavedSession> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_preserves_existing_name_and_finds_by_name_or_short_id() {
        let first = SavedSession {
            id: "019e778b-2c3f-7231-bda6-c40f27bbba21".to_string(),
            name: Some("main".to_string()),
            model: "gpt-5.5".to_string(),
            updated_at: "now".to_string(),
        };
        let second = SavedSession {
            id: first.id.clone(),
            name: None,
            model: "gpt-test".to_string(),
            updated_at: "later".to_string(),
        };

        let items = upsert_session(vec![first], second, "gpt-5.5");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name.as_deref(), Some("main"));
        assert_eq!(items[0].model, "gpt-test");
        assert!(find_session(&items, "main").is_some());
        assert!(find_session(&items, "019e778b").is_some());
    }

    #[test]
    fn reset_clears_session_and_increments_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), dir.path().join("cron"), "gpt-5.5".to_string());
        let key = SessionKey::Chat(42);

        assert_eq!(store.reset(&key).unwrap().generation, 1);
        let loaded = store.load(&key);

        assert_eq!(loaded.session_id, None);
        assert_eq!(loaded.model, "gpt-5.5");
        assert_eq!(loaded.generation, 1);
    }

    #[test]
    fn save_run_rejects_stale_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), dir.path().join("cron"), "gpt-5.5".to_string());
        let key = SessionKey::Cron("daily".to_string());

        store.reset(&key).unwrap();
        assert!(!store.save_run(&key, 0, "stale").unwrap());
        assert!(store.save_run(&key, 1, "fresh").unwrap());
        assert_eq!(store.load(&key).session_id.as_deref(), Some("fresh"));
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod config;
pub mod session;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test session::tests -- --nocapture`

Expected: session tests fail because session mutations return fixed errors.

- [ ] **Step 3: Implement session store**

Replace `src/session.rs` with:

```rust
use crate::text::session_label;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub generation: i64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SavedSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub model: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub enum SessionKey {
    Chat(i64),
    Cron(String),
}

pub struct SessionStore {
    chat_dir: PathBuf,
    cron_dir: PathBuf,
    default_model: String,
}

impl SessionStore {
    pub fn new(chat_dir: PathBuf, cron_dir: PathBuf, default_model: String) -> Self {
        Self {
            chat_dir,
            cron_dir,
            default_model,
        }
    }

    pub fn load(&self, key: &SessionKey) -> ChatSession {
        self.load_path(&self.path(key))
    }

    pub fn reset(&self, key: &SessionKey) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        state.session_id = None;
        state.generation += 1;
        state.updated_at = now_string();
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn set_model(&self, key: &SessionKey, model: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        state.model = model.trim().to_string();
        state.updated_at = now_string();
        if let Some(id) = state.session_id.clone() {
            state.sessions = upsert_session(
                state.sessions,
                SavedSession {
                    id,
                    name: None,
                    model: state.model.clone(),
                    updated_at: state.updated_at.clone(),
                },
                &self.default_model,
            );
        }
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn resume(&self, key: &SessionKey, target: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let found = find_session(&state.sessions, target)
            .ok_or_else(|| format!("No saved session matches \"{target}\"."))?;
        state.session_id = Some(found.id);
        if !found.model.is_empty() {
            state.model = found.model;
        }
        state.generation += 1;
        state.updated_at = now_string();
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn rename_current(&self, key: &SessionKey, name: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let id = state
            .session_id
            .clone()
            .ok_or_else(|| "No current session to rename. Send a normal message first.".to_string())?;
        state.updated_at = now_string();
        state.sessions = upsert_session(
            state.sessions,
            SavedSession {
                id,
                name: Some(name.trim().to_string()),
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            },
            &self.default_model,
        );
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn save_run(&self, key: &SessionKey, expected_generation: i64, session_id: &str) -> Result<bool, String> {
        let mut state = self.load(key);
        if state.generation != expected_generation {
            return Ok(false);
        }
        state.session_id = Some(session_id.to_string());
        state.updated_at = now_string();
        state.sessions = upsert_session(
            state.sessions,
            SavedSession {
                id: session_id.to_string(),
                name: None,
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            },
            &self.default_model,
        );
        self.save(key, &state)?;
        Ok(true)
    }

    pub fn list(&self, key: &SessionKey) -> String {
        let state = self.load(key);
        if state.sessions.is_empty() {
            return "No saved sessions yet. Send a normal message to create one.".to_string();
        }
        let mut lines = vec!["Saved sessions:".to_string()];
        for item in state.sessions {
            let marker = if Some(item.id.as_str()) == state.session_id.as_deref() {
                "*"
            } else {
                " "
            };
            let name = item.name.as_deref().unwrap_or("(unnamed)");
            let model = if item.model.is_empty() {
                self.default_model.as_str()
            } else {
                item.model.as_str()
            };
            lines.push(format!("{marker} {} {model} {name}", session_label(&item.id)));
        }
        lines.join("\n")
    }

    fn path(&self, key: &SessionKey) -> PathBuf {
        match key {
            SessionKey::Chat(chat_id) => self.chat_dir.join(format!("{chat_id}.json")),
            SessionKey::Cron(name) => self.cron_dir.join(format!("{}.json", sanitize_key(name))),
        }
    }

    fn load_path(&self, path: &Path) -> ChatSession {
        let mut state = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<ChatSession>(&text).ok())
            .unwrap_or_else(|| ChatSession {
                model: self.default_model.clone(),
                ..ChatSession::default()
            });
        if state.model.trim().is_empty() {
            state.model = self.default_model.clone();
        }
        if state.session_id.is_some() && state.sessions.is_empty() {
            state.sessions.push(SavedSession {
                id: state.session_id.clone().unwrap_or_default(),
                name: None,
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            });
        }
        state
    }

    fn save(&self, key: &SessionKey, state: &ChatSession) -> Result<(), String> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create session dir: {err}"))?;
        }
        let data = serde_json::to_vec_pretty(state).map_err(|err| err.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, [data, b"\n".to_vec()].concat()).map_err(|err| err.to_string())?;
        fs::rename(&tmp, &path).map_err(|err| err.to_string())
    }
}

pub fn upsert_session(mut items: Vec<SavedSession>, mut item: SavedSession, default_model: &str) -> Vec<SavedSession> {
    item.id = item.id.trim().to_string();
    if item.id.is_empty() {
        return items;
    }
    if item.model.trim().is_empty() {
        item.model = default_model.to_string();
    }
    for existing in &mut items {
        if existing.id == item.id {
            if item.name.is_none() {
                item.name = existing.name.clone();
            }
            *existing = item;
            return items;
        }
    }
    let mut out = vec![item];
    out.extend(items);
    out
}

pub fn find_session(items: &[SavedSession], target: &str) -> Option<SavedSession> {
    let target = target.trim();
    items
        .iter()
        .find(|item| item.id == target || session_label(&item.id) == target || item.name.as_deref() == Some(target))
        .cloned()
}

fn sanitize_key(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '_' })
        .collect()
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_preserves_existing_name_and_finds_by_name_or_short_id() {
        let first = SavedSession {
            id: "019e778b-2c3f-7231-bda6-c40f27bbba21".to_string(),
            name: Some("main".to_string()),
            model: "gpt-5.5".to_string(),
            updated_at: "now".to_string(),
        };
        let second = SavedSession {
            id: first.id.clone(),
            name: None,
            model: "gpt-test".to_string(),
            updated_at: "later".to_string(),
        };

        let items = upsert_session(vec![first], second, "gpt-5.5");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name.as_deref(), Some("main"));
        assert_eq!(items[0].model, "gpt-test");
        assert!(find_session(&items, "main").is_some());
        assert!(find_session(&items, "019e778b").is_some());
    }

    #[test]
    fn reset_clears_session_and_increments_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), dir.path().join("cron"), "gpt-5.5".to_string());
        let key = SessionKey::Chat(42);

        assert_eq!(store.reset(&key).unwrap().generation, 1);
        let loaded = store.load(&key);

        assert_eq!(loaded.session_id, None);
        assert_eq!(loaded.model, "gpt-5.5");
        assert_eq!(loaded.generation, 1);
    }

    #[test]
    fn save_run_rejects_stale_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), dir.path().join("cron"), "gpt-5.5".to_string());
        let key = SessionKey::Cron("daily".to_string());

        store.reset(&key).unwrap();
        assert!(!store.save_run(&key, 0, "stale").unwrap());
        assert!(store.save_run(&key, 1, "fresh").unwrap());
        assert_eq!(store.load(&key).session_id.as_deref(), Some("fresh"));
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test session::tests -- --nocapture`

Expected: all three session tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/session.rs
git commit -m "feat: add session store"
```

## Task 5: Codex Runner

**Files:**
- Create: `src/codex.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing Codex tests**

Create `src/codex.rs`:

```rust
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: PathBuf,
    pub home: PathBuf,
    pub workdir: PathBuf,
    pub default_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOutput {
    pub final_text: String,
    pub session_id: Option<String>,
}

pub fn codex_args(_out_path: &Path, _session_id: Option<&str>, _model: &str, _default_model: &str, _workdir: &Path) -> Vec<String> {
    Vec::new()
}

pub fn parse_codex_json(_output: &str) -> CodexOutput {
    CodexOutput { final_text: String::new(), session_id: None }
}

pub fn codex_env(_cfg: &CodexConfig) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_args_use_yolo_flag_for_new_session() {
        let args = codex_args(Path::new("/tmp/out"), None, "", "gpt-5.5", Path::new("/work"));
        let joined = args.join(" ");

        assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(joined.contains("--color never"));
        assert!(joined.contains("--cd /work"));
        assert!(!joined.contains("--ask-for-approval"));
        assert!(!joined.contains("--sandbox"));
    }

    #[test]
    fn codex_args_resume_session() {
        let args = codex_args(Path::new("/tmp/out"), Some("session-123"), "gpt-test", "gpt-5.5", Path::new("/work"));
        assert_eq!(
            args.join(" "),
            "exec resume --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m gpt-test --output-last-message /tmp/out session-123 -"
        );
    }

    #[test]
    fn parse_codex_json_extracts_thread_and_last_agent_message() {
        let output = r#"{"type":"thread.started","thread_id":"session-123"}
{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}
{"type":"item.completed","item":{"type":"agent_message","text":"bye"}}"#;

        assert_eq!(
            parse_codex_json(output),
            CodexOutput {
                session_id: Some("session-123".to_string()),
                final_text: "bye".to_string(),
            }
        );
    }

    #[test]
    fn codex_env_is_trimmed() {
        let cfg = CodexConfig {
            bin: PathBuf::from("/bin/codex"),
            home: PathBuf::from("/codex-home"),
            workdir: PathBuf::from("/work"),
            default_model: "gpt-5.5".to_string(),
        };

        let env = codex_env(&cfg);

        assert!(env.contains(&("CODEX_HOME".to_string(), "/codex-home".to_string())));
        assert!(env.contains(&("LANG".to_string(), "en_US.UTF-8".to_string())));
        assert!(env.iter().any(|(key, value)| key == "PATH" && value.contains("/opt/homebrew/bin")));
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod codex;
pub mod config;
pub mod session;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test codex::tests -- --nocapture`

Expected: Codex tests fail because args, env, and JSON parsing return empty values.

- [ ] **Step 3: Implement Codex pure functions**

Replace `src/codex.rs` with:

```rust
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: PathBuf,
    pub home: PathBuf,
    pub workdir: PathBuf,
    pub default_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOutput {
    pub final_text: String,
    pub session_id: Option<String>,
}

pub fn codex_args(
    out_path: &Path,
    session_id: Option<&str>,
    model: &str,
    default_model: &str,
    workdir: &Path,
) -> Vec<String> {
    let model = if model.trim().is_empty() { default_model } else { model.trim() };
    let out = out_path.to_string_lossy().to_string();
    let workdir = workdir.to_string_lossy().to_string();
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        return strings([
            "exec",
            "resume",
            "--json",
            "--skip-git-repo-check",
            "--dangerously-bypass-approvals-and-sandbox",
            "-m",
            model,
            "--output-last-message",
            &out,
            session_id,
            "-",
        ]);
    }

    strings([
        "exec",
        "--json",
        "--color",
        "never",
        "--cd",
        &workdir,
        "--skip-git-repo-check",
        "--dangerously-bypass-approvals-and-sandbox",
        "-m",
        model,
        "--output-last-message",
        &out,
        "-",
    ])
}

pub fn parse_codex_json(output: &str) -> CodexOutput {
    let mut result = CodexOutput {
        final_text: String::new(),
        session_id: None,
    };

    for line in output.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(event) = serde_json::from_str::<CodexEvent>(line) else {
            continue;
        };
        if event.event_type == "thread.started" {
            result.session_id = event.thread_id;
        } else if event.event_type == "item.completed" {
            if let Some(item) = event.item.filter(|item| item.item_type == "agent_message") {
                result.final_text = item.text.trim().to_string();
            }
        }
    }
    result
}

pub fn codex_env(cfg: &CodexConfig) -> Vec<(String, String)> {
    vec![
        ("HOME".to_string(), "/Users/example".to_string()),
        ("CODEX_HOME".to_string(), cfg.home.to_string_lossy().to_string()),
        (
            "PATH".to_string(),
            "/Users/example/.local/bin:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        ),
        ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
    ]
}

pub fn run_codex(cfg: &CodexConfig, prompt: &str, session_id: Option<&str>, model: &str, timeout: Duration, state_dir: &Path) -> Result<CodexOutput, String> {
    fs::create_dir_all(state_dir).map_err(|err| format!("create state dir: {err}"))?;
    let out_file = tempfile::NamedTempFile::new_in(state_dir).map_err(|err| err.to_string())?;
    let out_path = out_file.path().to_path_buf();
    let args = codex_args(&out_path, session_id, model, &cfg.default_model, &cfg.workdir);

    let mut child = Command::new(&cfg.bin)
        .args(args)
        .current_dir(&cfg.workdir)
        .env_clear()
        .envs(codex_env(cfg))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("start codex: {err}"))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "open codex stdin".to_string())?
        .write_all(prompt.as_bytes())
        .map_err(|err| format!("write codex stdin: {err}"))?;
    drop(child.stdin.take());

    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            let final_text = final_text_from_outputs(&out_path, &output.stdout, &output.stderr);
            return Err(format!("codex timed out after {timeout:?}\n\n{final_text}"));
        }
        if child.try_wait().map_err(|err| err.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            let parsed = parse_codex_json(&String::from_utf8_lossy(&output.stdout));
            let final_text = fs::read_to_string(&out_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            let final_text = if final_text.is_empty() { parsed.final_text } else { final_text };
            if output.status.success() {
                return Ok(CodexOutput {
                    final_text,
                    session_id: parsed.session_id,
                });
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err([final_text.as_str(), stderr.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn final_text_from_outputs(out_path: &Path, stdout: &[u8], stderr: &[u8]) -> String {
    let parsed = parse_codex_json(&String::from_utf8_lossy(stdout));
    let final_text = fs::read_to_string(out_path).unwrap_or_default();
    let stderr_text = String::from_utf8_lossy(stderr);
    [final_text.trim(), parsed.final_text.trim(), stderr_text.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[derive(Debug, Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    event_type: String,
    thread_id: Option<String>,
    item: Option<CodexItem>,
}

#[derive(Debug, Deserialize)]
struct CodexItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_args_use_yolo_flag_for_new_session() {
        let args = codex_args(Path::new("/tmp/out"), None, "", "gpt-5.5", Path::new("/work"));
        let joined = args.join(" ");

        assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(joined.contains("--color never"));
        assert!(joined.contains("--cd /work"));
        assert!(!joined.contains("--ask-for-approval"));
        assert!(!joined.contains("--sandbox"));
    }

    #[test]
    fn codex_args_resume_session() {
        let args = codex_args(Path::new("/tmp/out"), Some("session-123"), "gpt-test", "gpt-5.5", Path::new("/work"));
        assert_eq!(
            args.join(" "),
            "exec resume --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m gpt-test --output-last-message /tmp/out session-123 -"
        );
    }

    #[test]
    fn parse_codex_json_extracts_thread_and_last_agent_message() {
        let output = r#"{"type":"thread.started","thread_id":"session-123"}
{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}
{"type":"item.completed","item":{"type":"agent_message","text":"bye"}}"#;

        assert_eq!(
            parse_codex_json(output),
            CodexOutput {
                session_id: Some("session-123".to_string()),
                final_text: "bye".to_string(),
            }
        );
    }

    #[test]
    fn codex_env_is_trimmed() {
        let cfg = CodexConfig {
            bin: PathBuf::from("/bin/codex"),
            home: PathBuf::from("/codex-home"),
            workdir: PathBuf::from("/work"),
            default_model: "gpt-5.5".to_string(),
        };

        let env = codex_env(&cfg);

        assert!(env.contains(&("CODEX_HOME".to_string(), "/codex-home".to_string())));
        assert!(env.contains(&("LANG".to_string(), "en_US.UTF-8".to_string())));
        assert!(env.iter().any(|(key, value)| key == "PATH" && value.contains("/opt/homebrew/bin")));
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test codex::tests -- --nocapture`

Expected: all four Codex tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/lib.rs src/codex.rs
git commit -m "feat: add codex runner"
```

## Task 6: Telegram Client

**Files:**
- Create: `src/telegram.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing Telegram tests**

Create `src/telegram.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommandScope {
    #[serde(rename = "type")]
    pub scope_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandScopeTarget {
    pub name: String,
    pub scope: BotCommandScope,
    pub set: bool,
}

pub fn supported_bot_commands() -> Vec<BotCommand> {
    Vec::new()
}

pub fn command_scope_targets(_chat_ids: &[i64]) -> Vec<CommandScopeTarget> {
    Vec::new()
}

pub fn command_request_values(_scope: &BotCommandScope, _language_code: &str) -> Result<Vec<(String, String)>, String> {
    Ok(Vec::new())
}

pub fn redact_token(_base_url: &str, value: &str) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_commands_match_directives() {
        let commands = supported_bot_commands();
        let names: Vec<_> = commands.iter().map(|command| command.command.as_str()).collect();

        assert_eq!(names, vec!["commands", "help", "status", "log", "new", "restart", "model", "resume", "rename", "list"]);
    }

    #[test]
    fn command_scope_targets_match_go_gateway() {
        let targets = command_scope_targets(&[<telegram_chat_id>]);
        let summary: Vec<_> = targets.iter().map(|target| (target.name.as_str(), target.scope.scope_type.as_str(), target.set)).collect();

        assert_eq!(
            summary,
            vec![
                ("default", "default", true),
                ("all_private_chats", "all_private_chats", true),
                ("all_group_chats", "all_group_chats", false),
                ("all_chat_administrators", "all_chat_administrators", false),
                ("chat:<telegram_chat_id>", "chat", true),
            ]
        );
        assert_eq!(targets[4].scope.chat_id, Some(<telegram_chat_id>));
    }

    #[test]
    fn command_request_values_encode_scope_and_language() {
        let values = command_request_values(
            &BotCommandScope {
                scope_type: "chat".to_string(),
                chat_id: Some(<telegram_chat_id>),
            },
            "en",
        )
        .unwrap();

        assert!(values.contains(&("scope".to_string(), r#"{"type":"chat","chat_id":<telegram_chat_id>}"#.to_string())));
        assert!(values.contains(&("language_code".to_string(), "en".to_string())));
    }

    #[test]
    fn redact_token_hides_base_url() {
        let base = "https://api.telegram.org/botsecret";
        assert_eq!(
            redact_token(base, "request to https://api.telegram.org/botsecret/getUpdates failed"),
            "request to https://api.telegram.org/bot<redacted>/getUpdates failed"
        );
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod codex;
pub mod config;
pub mod session;
pub mod telegram;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test telegram::tests -- --nocapture`

Expected: Telegram tests fail because the command and scope functions return empty values.

- [ ] **Step 3: Implement Telegram DTOs and pure helpers**

Replace `src/telegram.rs` with:

```rust
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotCommandScope {
    #[serde(rename = "type")]
    pub scope_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandScopeTarget {
    pub name: String,
    pub scope: BotCommandScope,
    pub set: bool,
}

#[derive(Debug, Deserialize)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub caption: String,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub username: String,
}

pub struct TelegramClient {
    base_url: String,
    agent: ureq::Agent,
}

impl TelegramClient {
    pub fn new(token: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(65))
            .build();
        Self {
            base_url: format!("https://api.telegram.org/bot{token}"),
            agent,
        }
    }

    pub fn get_updates(&self, offset: i64, timeout_sec: u64) -> Result<Vec<Update>, String> {
        let mut request = self
            .agent
            .get(&format!("{}/getUpdates", self.base_url))
            .query("timeout", &timeout_sec.to_string())
            .query("allowed_updates", r#"["message"]"#);
        if offset > 0 {
            request = request.query("offset", &offset.to_string());
        }
        self.call_json(request, "getUpdates")
    }

    pub fn send_message(&self, chat_id: i64, text: &str, reply_to_message_id: i64) -> Result<(), String> {
        let mut values = vec![
            ("chat_id", chat_id.to_string()),
            ("text", text.to_string()),
            ("disable_web_page_preview", "true".to_string()),
        ];
        if reply_to_message_id > 0 {
            values.push(("reply_to_message_id", reply_to_message_id.to_string()));
            values.push(("allow_sending_without_reply", "true".to_string()));
        }
        let _: serde_json::Value = self.post_form("sendMessage", &values)?;
        Ok(())
    }

    pub fn send_chat_action(&self, chat_id: i64, action: &str) -> Result<(), String> {
        let values = [("chat_id", chat_id.to_string()), ("action", action.to_string())];
        let _: bool = self.post_form("sendChatAction", &values)?;
        Ok(())
    }

    pub fn sync_my_commands(&self, chat_ids: &[i64]) -> Result<(), String> {
        let languages = ["", "en", "he"];
        let targets = command_scope_targets(chat_ids);
        for target in &targets {
            for language in languages {
                self.delete_my_commands(&target.scope, language)?;
            }
        }
        for target in targets.iter().filter(|target| target.set) {
            for language in languages {
                self.set_my_commands(&target.scope, language)?;
            }
        }
        Ok(())
    }

    fn delete_my_commands(&self, scope: &BotCommandScope, language_code: &str) -> Result<(), String> {
        let values = command_request_values(scope, language_code)?;
        let refs = refs(&values);
        let _: bool = self.post_form("deleteMyCommands", &refs)?;
        Ok(())
    }

    fn set_my_commands(&self, scope: &BotCommandScope, language_code: &str) -> Result<(), String> {
        let mut values = command_request_values(scope, language_code)?;
        values.push((
            "commands".to_string(),
            serde_json::to_string(&supported_bot_commands()).map_err(|err| err.to_string())?,
        ));
        let refs = refs(&values);
        let _: bool = self.post_form("setMyCommands", &refs)?;
        Ok(())
    }

    fn post_form<T: serde::de::DeserializeOwned>(&self, method: &str, values: &[(&str, String)]) -> Result<T, String> {
        let form: Vec<(&str, &str)> = values.iter().map(|(key, value)| (*key, value.as_str())).collect();
        self.call_json(self.agent.post(&format!("{}/{method}", self.base_url)).send_form(&form), method)
    }

    fn call_json<T: serde::de::DeserializeOwned, R: IntoResponse>(&self, response: R, method: &str) -> Result<T, String> {
        let response = response.into_response().map_err(|err| format!("telegram {method} request failed: {}", redact_token(&self.base_url, &err.to_string())))?;
        let envelope: TelegramResponse<T> = response.into_json().map_err(|err| format!("decode telegram {method} response: {err}"))?;
        if envelope.ok {
            envelope.result.ok_or_else(|| format!("telegram {method} returned no result"))
        } else {
            Err(envelope.description.unwrap_or_else(|| format!("telegram {method} failed")))
        }
    }
}

trait IntoResponse {
    fn into_response(self) -> Result<ureq::Response, ureq::Error>;
}

impl IntoResponse for ureq::Request {
    fn into_response(self) -> Result<ureq::Response, ureq::Error> {
        self.call()
    }
}

impl IntoResponse for Result<ureq::Response, ureq::Error> {
    fn into_response(self) -> Result<ureq::Response, ureq::Error> {
        self
    }
}

pub fn supported_bot_commands() -> Vec<BotCommand> {
    [
        ("commands", "Show supported gateway directives."),
        ("help", "Alias for /commands."),
        ("status", "Show gateway status and system snapshot."),
        ("log", "Send recent gateway logs."),
        ("new", "Start a fresh Codex session."),
        ("restart", "Restart the gateway service."),
        ("model", "Show or set the Codex model."),
        ("resume", "Resume a saved session."),
        ("rename", "Rename the current session."),
        ("list", "List saved sessions."),
    ]
    .into_iter()
    .map(|(command, description)| BotCommand {
        command: command.to_string(),
        description: description.to_string(),
    })
    .collect()
}

pub fn command_scope_targets(chat_ids: &[i64]) -> Vec<CommandScopeTarget> {
    let mut targets = vec![
        target("default", "default", None, true),
        target("all_private_chats", "all_private_chats", None, true),
        target("all_group_chats", "all_group_chats", None, false),
        target("all_chat_administrators", "all_chat_administrators", None, false),
    ];
    for chat_id in chat_ids {
        targets.push(target(&format!("chat:{chat_id}"), "chat", Some(*chat_id), true));
    }
    targets
}

pub fn command_request_values(scope: &BotCommandScope, language_code: &str) -> Result<Vec<(String, String)>, String> {
    let mut values = vec![("scope".to_string(), serde_json::to_string(scope).map_err(|err| err.to_string())?)];
    if !language_code.is_empty() {
        values.push(("language_code".to_string(), language_code.to_string()));
    }
    Ok(values)
}

pub fn redact_token(base_url: &str, value: &str) -> String {
    value.replace(base_url, "https://api.telegram.org/bot<redacted>")
}

fn target(name: &str, scope_type: &str, chat_id: Option<i64>, set: bool) -> CommandScopeTarget {
    CommandScopeTarget {
        name: name.to_string(),
        scope: BotCommandScope {
            scope_type: scope_type.to_string(),
            chat_id,
        },
        set,
    }
}

fn refs(values: &[(String, String)]) -> Vec<(&str, String)> {
    values.iter().map(|(key, value)| (key.as_str(), value.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_commands_match_directives() {
        let commands = supported_bot_commands();
        let names: Vec<_> = commands.iter().map(|command| command.command.as_str()).collect();

        assert_eq!(names, vec!["commands", "help", "status", "log", "new", "restart", "model", "resume", "rename", "list"]);
    }

    #[test]
    fn command_scope_targets_match_go_gateway() {
        let targets = command_scope_targets(&[<telegram_chat_id>]);
        let summary: Vec<_> = targets.iter().map(|target| (target.name.as_str(), target.scope.scope_type.as_str(), target.set)).collect();

        assert_eq!(
            summary,
            vec![
                ("default", "default", true),
                ("all_private_chats", "all_private_chats", true),
                ("all_group_chats", "all_group_chats", false),
                ("all_chat_administrators", "all_chat_administrators", false),
                ("chat:<telegram_chat_id>", "chat", true),
            ]
        );
        assert_eq!(targets[4].scope.chat_id, Some(<telegram_chat_id>));
    }

    #[test]
    fn command_request_values_encode_scope_and_language() {
        let values = command_request_values(
            &BotCommandScope {
                scope_type: "chat".to_string(),
                chat_id: Some(<telegram_chat_id>),
            },
            "en",
        )
        .unwrap();

        assert!(values.contains(&("scope".to_string(), r#"{"type":"chat","chat_id":<telegram_chat_id>}"#.to_string())));
        assert!(values.contains(&("language_code".to_string(), "en".to_string())));
    }

    #[test]
    fn redact_token_hides_base_url() {
        let base = "https://api.telegram.org/botsecret";
        assert_eq!(
            redact_token(base, "request to https://api.telegram.org/botsecret/getUpdates failed"),
            "request to https://api.telegram.org/bot<redacted>/getUpdates failed"
        );
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test telegram::tests -- --nocapture`

Expected: all four Telegram tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/telegram.rs
git commit -m "feat: add telegram client"
```

## Task 7: Command Decisions and Status

**Files:**
- Create: `src/commands.rs`
- Create: `src/status.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing command and status tests**

Create `src/status.rs`:

```rust
use crate::session::ChatSession;

pub fn status_header(_state: &ChatSession) -> String {
    String::new()
}

pub fn format_status_message(_state: &ChatSession, _fetch: &str) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn format_status_message_appends_fetch() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = format_status_message(&state, "OS: test");

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("OS: test"));
        assert!(!got.contains("Gateway restarted."));
    }
}
```

Create `src/commands.rs`:

```rust
pub const DIRECTIVES: &str = "/commands, /help, /status, /log, /new, /restart, /model, /resume, /rename, /list";

pub fn directive_help() -> String {
    String::new()
}

pub fn unknown_directive_message() -> String {
    String::new()
}

pub fn is_allowed(_allowed_ids: &[i64], _chat_id: i64, _from_id: Option<i64>) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_help_includes_supported_commands() {
        let help = directive_help();
        for command in ["/commands", "/help", "/status", "/log", "/new", "/restart", "/model", "/resume", "/rename", "/list"] {
            assert!(help.contains(command), "missing {command}");
        }
        assert!(!help.contains("/start"));
    }

    #[test]
    fn unknown_directive_mentions_defined_directives() {
        let message = unknown_directive_message();
        assert!(message.contains("Unknown directive."));
        assert!(message.contains(DIRECTIVES));
    }

    #[test]
    fn is_allowed_accepts_chat_or_sender() {
        assert!(is_allowed(&[42], 42, None));
        assert!(is_allowed(&[42], 7, Some(42)));
        assert!(!is_allowed(&[42], 7, Some(8)));
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod codex;
pub mod commands;
pub mod config;
pub mod session;
pub mod status;
pub mod telegram;
pub mod text;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo test commands::tests -- --nocapture
cargo test status::tests -- --nocapture
```

Expected: command and status tests fail because functions return empty/default values.

- [ ] **Step 3: Implement command and status functions**

Replace `src/commands.rs` with:

```rust
pub const DIRECTIVES: &str = "/commands, /help, /status, /log, /new, /restart, /model, /resume, /rename, /list";

pub fn directive_help() -> String {
    [
        "Supported directives:",
        "/commands - show supported gateway directives",
        "/help - alias for /commands",
        "/status - show gateway status and system snapshot",
        "/log [lines] - send recent gateway logs",
        "/new - start a fresh Codex session",
        "/restart - restart the gateway service",
        "/model [name] - show or set the Codex model",
        "/resume SESSION_OR_NAME - resume a saved session",
        "/rename NAME - rename the current session",
        "/list - list saved sessions",
    ]
    .join("\n")
}

pub fn unknown_directive_message() -> String {
    format!("Unknown directive. Defined directives: {DIRECTIVES}")
}

pub fn is_allowed(allowed_ids: &[i64], chat_id: i64, from_id: Option<i64>) -> bool {
    allowed_ids.contains(&chat_id) || from_id.is_some_and(|id| allowed_ids.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_help_includes_supported_commands() {
        let help = directive_help();
        for command in ["/commands", "/help", "/status", "/log", "/new", "/restart", "/model", "/resume", "/rename", "/list"] {
            assert!(help.contains(command), "missing {command}");
        }
        assert!(!help.contains("/start"));
    }

    #[test]
    fn unknown_directive_mentions_defined_directives() {
        let message = unknown_directive_message();
        assert!(message.contains("Unknown directive."));
        assert!(message.contains(DIRECTIVES));
    }

    #[test]
    fn is_allowed_accepts_chat_or_sender() {
        assert!(is_allowed(&[42], 42, None));
        assert!(is_allowed(&[42], 7, Some(42)));
        assert!(!is_allowed(&[42], 7, Some(8)));
    }
}
```

Replace `src/status.rs` with:

```rust
use crate::session::ChatSession;
use crate::text::session_label;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub fn status_header(state: &ChatSession) -> String {
    format!(
        "Model: {}\nSession: {}",
        state.model,
        session_label(state.session_id.as_deref().unwrap_or(""))
    )
}

pub fn format_status_message(state: &ChatSession, fetch: &str) -> String {
    let fetch = fetch.trim();
    if fetch.is_empty() {
        return status_header(state);
    }
    format!("{}\n\n{fetch}", status_header(state))
}

pub fn fastfetch_status(bin: &Path) -> String {
    let output = Command::new(bin)
        .args([
            "--logo",
            "none",
            "--pipe",
            "--structure",
            "OS:Host:Kernel:Uptime:CPU:GPU:Memory:Swap:Disk:Battery:LocalIp",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() { "fastfetch: no output".to_string() } else { text }
        }
        Ok(output) => format!("fastfetch: exited with {}", output.status),
        Err(err) => format!("fastfetch: {err}"),
    }
}

pub fn typing_interval() -> Duration {
    Duration::from_secs(4)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn format_status_message_appends_fetch() {
        let state = ChatSession {
            session_id: Some("12345678".to_string()),
            model: "gpt-test".to_string(),
            ..ChatSession::default()
        };

        let got = format_status_message(&state, "OS: test");

        assert!(got.contains("Model: gpt-test"));
        assert!(got.contains("OS: test"));
        assert!(!got.contains("Gateway restarted."));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cargo test commands::tests -- --nocapture
cargo test status::tests -- --nocapture
```

Expected: all five command and status tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/commands.rs src/status.rs
git commit -m "feat: add command decisions and status formatting"
```

## Task 8: Run Mode

**Files:**
- Create: `src/run_mode.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing run-mode tests**

Create `src/run_mode.rs`:

```rust
use crate::cli::RunArgs;
use std::io::Read;

pub fn load_prompt<R: Read>(_args: &RunArgs, _stdin: R) -> Result<String, String> {
    Err("missing behavior".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn base_args() -> RunArgs {
        RunArgs {
            job: "daily".to_string(),
            prompt: None,
            prompt_file: None,
            model: None,
            new_session: false,
            telegram_chat: None,
        }
    }

    #[test]
    fn prompt_argument_wins_over_stdin() {
        let mut args = base_args();
        args.prompt = Some("from arg".to_string());

        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from arg");
    }

    #[test]
    fn prompt_file_wins_over_stdin() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "from file").unwrap();
        let mut args = base_args();
        args.prompt_file = Some(file.path().to_path_buf());

        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from file");
    }

    #[test]
    fn stdin_is_used_when_no_prompt_source_is_given() {
        let args = base_args();
        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from stdin");
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let args = base_args();
        let err = load_prompt(&args, Cursor::new("   ")).unwrap_err();
        assert!(err.contains("prompt is empty"));
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod cli;
pub mod codex;
pub mod commands;
pub mod config;
pub mod run_mode;
pub mod session;
pub mod status;
pub mod telegram;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test run_mode::tests -- --nocapture`

Expected: run-mode tests fail because `load_prompt` returns `Err("missing behavior")`.

- [ ] **Step 3: Implement prompt loading and run orchestration**

Replace `src/run_mode.rs` with:

```rust
use crate::cli::RunArgs;
use crate::codex::{run_codex, CodexConfig};
use crate::config::Config;
use crate::session::{SessionKey, SessionStore};
use crate::telegram::TelegramClient;
use crate::text::split_telegram_message;
use std::fs;
use std::io::Read;

pub fn load_prompt<R: Read>(args: &RunArgs, mut stdin: R) -> Result<String, String> {
    let prompt = match (&args.prompt, &args.prompt_file) {
        (Some(prompt), _) => prompt.clone(),
        (None, Some(path)) => fs::read_to_string(path).map_err(|err| format!("read prompt file: {err}"))?,
        (None, None) => {
            let mut text = String::new();
            stdin.read_to_string(&mut text).map_err(|err| format!("read stdin: {err}"))?;
            text
        }
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("prompt is empty".to_string());
    }
    Ok(prompt)
}

pub fn run(args: RunArgs, cfg: Config) -> Result<String, String> {
    let prompt = load_prompt(&args, std::io::stdin())?;
    let store = SessionStore::new(cfg.chat_state_dir.clone(), cfg.cron_state_dir.clone(), cfg.codex_model.clone());
    let key = SessionKey::Cron(args.job.clone());
    if args.new_session {
        store.reset(&key)?;
    }
    let state = store.load(&key);
    let model = args.model.as_deref().unwrap_or(&state.model);
    let output = run_codex(
        &CodexConfig {
            bin: cfg.codex_bin.clone(),
            home: cfg.codex_home.clone(),
            workdir: cfg.codex_workdir.clone(),
            default_model: cfg.codex_model.clone(),
        },
        &prompt,
        state.session_id.as_deref(),
        model,
        cfg.codex_timeout,
        &cfg.state_dir,
    )?;
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_run(&key, state.generation, session_id)?;
    }
    if let Some(chat_id) = args.telegram_chat {
        let tg = TelegramClient::new(&cfg.bot_token);
        for part in split_telegram_message(&output.final_text) {
            tg.send_message(chat_id, &part, 0)?;
        }
    }
    Ok(output.final_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn base_args() -> RunArgs {
        RunArgs {
            job: "daily".to_string(),
            prompt: None,
            prompt_file: None,
            model: None,
            new_session: false,
            telegram_chat: None,
        }
    }

    #[test]
    fn prompt_argument_wins_over_stdin() {
        let mut args = base_args();
        args.prompt = Some("from arg".to_string());

        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from arg");
    }

    #[test]
    fn prompt_file_wins_over_stdin() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "from file").unwrap();
        let mut args = base_args();
        args.prompt_file = Some(file.path().to_path_buf());

        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from file");
    }

    #[test]
    fn stdin_is_used_when_no_prompt_source_is_given() {
        let args = base_args();
        assert_eq!(load_prompt(&args, Cursor::new("from stdin")).unwrap(), "from stdin");
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let args = base_args();
        let err = load_prompt(&args, Cursor::new("   ")).unwrap_err();
        assert!(err.contains("prompt is empty"));
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test run_mode::tests -- --nocapture`

Expected: run-mode tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/run_mode.rs
git commit -m "feat: add cron run mode"
```

## Task 9: Bot Orchestration

**Files:**
- Create: `src/bot.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write failing bot tests**

Create `src/bot.rs`:

```rust
use std::path::Path;

pub fn read_offset(_path: &Path) -> i64 {
    0
}

pub fn write_offset(_path: &Path, _offset: i64) -> Result<(), String> {
    Err("missing behavior".to_string())
}

pub fn message_text(_text: &str, _caption: &str) -> Result<String, String> {
    Err("missing behavior".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn offset_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");

        write_offset(&path, 42).unwrap();

        assert_eq!(read_offset(&path), 42);
    }

    #[test]
    fn invalid_offset_returns_zero() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");
        std::fs::write(&path, "bad").unwrap();

        assert_eq!(read_offset(&path), 0);
    }

    #[test]
    fn message_text_prefers_text_then_caption() {
        assert_eq!(message_text(" hello ", "caption").unwrap(), "hello");
        assert_eq!(message_text("", " caption ").unwrap(), "caption");
        assert_eq!(message_text("", "").unwrap_err(), "Text messages only.");
    }
}
```

Modify `src/lib.rs`:

```rust
pub mod bot;
pub mod cli;
pub mod codex;
pub mod commands;
pub mod config;
pub mod run_mode;
pub mod session;
pub mod status;
pub mod telegram;
pub mod text;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test bot::tests -- --nocapture`

Expected: bot tests fail because `write_offset` and `message_text` return errors.

- [ ] **Step 3: Implement bot helpers and orchestration**

Replace `src/bot.rs` with:

```rust
use crate::codex::{run_codex, CodexConfig};
use crate::commands::{directive_help, is_allowed, unknown_directive_message};
use crate::config::Config;
use crate::session::{SessionKey, SessionStore};
use crate::status::{fastfetch_status, format_status_message};
use crate::telegram::{Message, TelegramClient};
use crate::text::{command_arg, log_line_count, parse_command, split_telegram_message, tail_log_text};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
struct Job {
    chat_id: i64,
    reply_to_message_id: i64,
    prompt: String,
}

pub fn run(cfg: Config) -> Result<(), String> {
    fs::create_dir_all(&cfg.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    fs::create_dir_all(&cfg.chat_state_dir).map_err(|err| format!("create chat state dir: {err}"))?;
    fs::create_dir_all(&cfg.cron_state_dir).map_err(|err| format!("create cron state dir: {err}"))?;

    let tg = TelegramClient::new(&cfg.bot_token);
    tg.sync_my_commands(&cfg.allowed_ids)?;
    let store = SessionStore::new(cfg.chat_state_dir.clone(), cfg.cron_state_dir.clone(), cfg.codex_model.clone());
    for chat_id in &cfg.allowed_ids {
        let state = store.load(&SessionKey::Chat(*chat_id));
        send_long_message(&tg, *chat_id, &format_status_message(&state, &fastfetch_status(&cfg.fastfetch_bin)), 0)?;
    }

    let (tx, rx) = mpsc::sync_channel::<Job>(cfg.queue_depth);
    let worker_cfg = cfg.clone();
    let _worker = thread::spawn(move || worker_loop(worker_cfg, rx));

    let mut offset = read_offset(&cfg.offset_file);
    loop {
        let updates = tg.get_updates(offset, cfg.poll_timeout_sec)?;
        for update in updates {
            offset = offset.max(update.update_id + 1);
            write_offset(&cfg.offset_file, offset)?;
            if let Some(message) = update.message {
                handle_message(&cfg, &tg, &store, &tx, message)?;
            }
        }
    }
}

pub fn read_offset(path: &Path) -> i64 {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

pub fn write_offset(path: &Path, offset: i64) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let tmp = path.with_extension("offset.tmp");
    fs::write(&tmp, format!("{offset}\n")).map_err(|err| err.to_string())?;
    fs::rename(&tmp, path).map_err(|err| err.to_string())
}

pub fn message_text(text: &str, caption: &str) -> Result<String, String> {
    let text = text.trim();
    if !text.is_empty() {
        return Ok(text.to_string());
    }
    let caption = caption.trim();
    if !caption.is_empty() {
        return Ok(caption.to_string());
    }
    Err("Text messages only.".to_string())
}

fn handle_message(cfg: &Config, tg: &TelegramClient, store: &SessionStore, tx: &mpsc::SyncSender<Job>, msg: Message) -> Result<(), String> {
    let from_id = msg.from.as_ref().map(|user| user.id);
    if !is_allowed(&cfg.allowed_ids, msg.chat.id, from_id) {
        return Ok(());
    }
    let text = match message_text(&msg.text, &msg.caption) {
        Ok(text) => text,
        Err(err) => {
            tg.send_message(msg.chat.id, &err, msg.message_id)?;
            return Ok(());
        }
    };

    if let Some(command) = parse_command(&text) {
        return handle_command(cfg, tg, store, &msg, &text, &command);
    }

    let queued = tx.try_send(Job {
        chat_id: msg.chat.id,
        reply_to_message_id: msg.message_id,
        prompt: text,
    });
    if queued.is_err() {
        tg.send_message(msg.chat.id, "Codex queue is full. Try again after the current requests finish.", msg.message_id)?;
    }
    Ok(())
}

fn handle_command(cfg: &Config, tg: &TelegramClient, store: &SessionStore, msg: &Message, text: &str, command: &str) -> Result<(), String> {
    let key = SessionKey::Chat(msg.chat.id);
    match command {
        "/log" => {
            let lines = log_line_count(text);
            let body = fs::read_to_string(&cfg.gateway_log_file)
                .map(|log_text| tail_log_text(&log_text, lines))
                .unwrap_or_else(|_| "No gateway log available.".to_string());
            send_long_message(tg, msg.chat.id, &body, msg.message_id)
        }
        "/new" => {
            let state = store.reset(&key)?;
            tg.send_message(msg.chat.id, &format!("New session ready. Model: {}", state.model), msg.message_id)
        }
        "/restart" => {
            tg.send_message(msg.chat.id, "Restarting gateway.", msg.message_id)?;
            restart_gateway(&cfg.launchd_target);
            Ok(())
        }
        "/model" => {
            let model = command_arg(text);
            if model.is_empty() {
                let state = store.load(&key);
                return tg.send_message(msg.chat.id, &crate::status::status_header(&state), msg.message_id);
            }
            let state = store.set_model(&key, &model)?;
            tg.send_message(msg.chat.id, &format!("Model set to {}\nSession: {}", state.model, crate::text::session_label(state.session_id.as_deref().unwrap_or(""))), msg.message_id)
        }
        "/resume" => {
            let target = command_arg(text);
            if target.is_empty() {
                let body = format!("Usage: /resume SESSION_OR_NAME\n\n{}", store.list(&key));
                return send_long_message(tg, msg.chat.id, &body, msg.message_id);
            }
            let state = store.resume(&key, &target)?;
            tg.send_message(msg.chat.id, &format!("Resumed session {}\nModel: {}", crate::text::session_label(state.session_id.as_deref().unwrap_or("")), state.model), msg.message_id)
        }
        "/rename" => {
            let name = command_arg(text);
            if name.is_empty() {
                return tg.send_message(msg.chat.id, "Usage: /rename NAME", msg.message_id);
            }
            let state = store.rename_current(&key, &name)?;
            tg.send_message(msg.chat.id, &format!("Renamed session {} to \"{name}\".", crate::text::session_label(state.session_id.as_deref().unwrap_or(""))), msg.message_id)
        }
        "/list" => send_long_message(tg, msg.chat.id, &store.list(&key), msg.message_id),
        "/help" | "/commands" => tg.send_message(msg.chat.id, &directive_help(), msg.message_id),
        "/status" => {
            let state = store.load(&key);
            send_long_message(tg, msg.chat.id, &format_status_message(&state, &fastfetch_status(&cfg.fastfetch_bin)), msg.message_id)
        }
        _ => tg.send_message(msg.chat.id, &unknown_directive_message(), msg.message_id),
    }
}

fn worker_loop(cfg: Config, rx: mpsc::Receiver<Job>) {
    let tg = TelegramClient::new(&cfg.bot_token);
    let store = SessionStore::new(cfg.chat_state_dir.clone(), cfg.cron_state_dir.clone(), cfg.codex_model.clone());
    for job in rx {
        let _ = run_job(&cfg, &tg, &store, job);
    }
}

fn run_job(cfg: &Config, tg: &TelegramClient, store: &SessionStore, job: Job) -> Result<(), String> {
    let key = SessionKey::Chat(job.chat_id);
    let state = store.load(&key);
    let output = match run_codex(
        &CodexConfig {
            bin: cfg.codex_bin.clone(),
            home: cfg.codex_home.clone(),
            workdir: cfg.codex_workdir.clone(),
            default_model: cfg.codex_model.clone(),
        },
        &job.prompt,
        state.session_id.as_deref(),
        &state.model,
        cfg.codex_timeout,
        &cfg.state_dir,
    ) {
        Ok(output) => output,
        Err(err) => {
            send_long_message(tg, job.chat_id, &format!("Codex failed:\n{err}"), job.reply_to_message_id)?;
            return Ok(());
        }
    };
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_run(&key, state.generation, session_id)?;
    }
    send_long_message(tg, job.chat_id, &empty_final_text(&output.final_text), job.reply_to_message_id)
}

fn empty_final_text(text: &str) -> String {
    if text.trim().is_empty() {
        "Codex finished with no final text.".to_string()
    } else {
        text.to_string()
    }
}

fn send_long_message(tg: &TelegramClient, chat_id: i64, text: &str, reply_to_message_id: i64) -> Result<(), String> {
    for (index, part) in split_telegram_message(text).into_iter().enumerate() {
        let reply = if index == 0 { reply_to_message_id } else { 0 };
        tg.send_message(chat_id, &part, reply)?;
    }
    Ok(())
}

fn restart_gateway(launchd_target: &str) {
    let _ = Command::new("/bin/launchctl")
        .args(["kickstart", "-k", launchd_target])
        .spawn();
}

pub fn typing_sleep() -> Duration {
    Duration::from_secs(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn offset_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");

        write_offset(&path, 42).unwrap();

        assert_eq!(read_offset(&path), 42);
    }

    #[test]
    fn invalid_offset_returns_zero() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("telegram.offset");
        std::fs::write(&path, "bad").unwrap();

        assert_eq!(read_offset(&path), 0);
    }

    #[test]
    fn message_text_prefers_text_then_caption() {
        assert_eq!(message_text(" hello ", "caption").unwrap(), "hello");
        assert_eq!(message_text("", " caption ").unwrap(), "caption");
        assert_eq!(message_text("", "").unwrap_err(), "Text messages only.");
    }
}
```

Replace `src/main.rs` with:

```rust
use gateway::cli::{parse_args_from, Mode};

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mode = parse_args_from(std::env::args_os())?;
    let cfg = gateway::config::load()?;
    match mode {
        Mode::Bot => gateway::bot::run(cfg),
        Mode::Run(args) => {
            let output = gateway::run_mode::run(args, cfg)?;
            println!("{output}");
            Ok(())
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test bot::tests -- --nocapture`

Expected: all three bot helper tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/main.rs src/bot.rs
git commit -m "feat: add telegram bot mode"
```

## Task 10: Launch Files and README

**Files:**
- Create: `ai.gateway.plist`
- Create: `gateway-launch.sh`
- Create: `README.md`

- [ ] **Step 1: Write launch and README content**

Create `ai.gateway.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>ai.gateway</string>
    <key>Comment</key>
    <string>Telegram to Codex Gateway</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>1</integer>
    <key>Umask</key>
    <integer>63</integer>
    <key>ProgramArguments</key>
    <array>
      <string>/bin/zsh</string>
      <string>/Users/example/.config/gateway/gateway-launch.sh</string>
    </array>
    <key>StandardOutPath</key>
    <string>/Users/example/.local/share/gateway/logs/gateway.log</string>
    <key>StandardErrorPath</key>
    <string>/Users/example/.local/share/gateway/logs/gateway.err.log</string>
  </dict>
</plist>
```

Create `gateway-launch.sh`:

```zsh
#!/bin/zsh
set -euo pipefail

source /Users/example/$XDG_CONFIG_HOME/gateway/env

: "${TELEGRAM_BOT_TOKEN:?TELEGRAM_BOT_TOKEN is required}"

exec env -i \
  HOME=/Users/example \
  TELEGRAM_BOT_TOKEN="$TELEGRAM_BOT_TOKEN" \
  /Users/example/.local/bin/gateway bot
```

Create `README.md`:

````markdown
# Gateway

Lean Rust Telegram-to-Codex gateway.

## Build

```sh
cargo test
cargo build --release
install -m 755 target/release/gateway "$HOME/.local/bin/gateway"
```

## Bot

`gateway bot` long-polls Telegram, accepts allowlisted text messages, runs
`codex exec`, and replies with the final answer.

Required secret:

```sh
export TELEGRAM_BOT_TOKEN=...
```

Useful overrides:

```sh
export GATEWAY_ALLOWED_IDS=<telegram_chat_id>
export GATEWAY_CODEX_MODEL=gpt-5.5
export GATEWAY_STATE_DIR=/Users/example/.local/state/gateway
```

## Cron or launchd jobs

Run a one-shot Codex prompt without touching the chat bot process:

```sh
gateway run --job daily --prompt "Summarize the current system state"
```

Send the result to Telegram:

```sh
gateway run --job daily --prompt-file /path/to/prompt.txt --telegram-chat <telegram_chat_id>
```

## LaunchAgent

```sh
mkdir -p "$HOME/.local/share/gateway/logs"
cp ai.gateway.plist "$HOME/Library/LaunchAgents/ai.gateway.plist"
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/ai.gateway.plist"
launchctl kickstart -k "gui/$(id -u)/ai.gateway"
```
````

- [ ] **Step 2: Make launcher executable**

Run: `chmod +x gateway-launch.sh`

Expected: `gateway-launch.sh` becomes executable.

- [ ] **Step 3: Verify shell syntax**

Run: `zsh -n gateway-launch.sh`

Expected: exits 0 with no output.

- [ ] **Step 4: Commit**

```bash
git add ai.gateway.plist gateway-launch.sh README.md
git commit -m "docs: add gateway launch instructions"
```

## Task 11: Final Verification and Cleanup

**Files:**
- Modify: any Rust file that fails formatting, clippy, or tests.

- [ ] **Step 1: Run full tests**

Run: `cargo test`

Expected: every unit test passes.

- [ ] **Step 2: Run formatter check**

Run: `cargo fmt --check`

Expected: exits 0. If it fails, run `cargo fmt`, inspect `git diff`, and commit formatting with the implementation commit that introduced the formatting issue.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`

Expected: exits 0. Fix each warning by simplifying the code or tightening types; do not suppress warnings unless the lint is demonstrably wrong for this code.

- [ ] **Step 4: Run cleanup pass**

Inspect the modules for duplicated logic and oversized functions:

```bash
rg -n "unwrap\\(|expect\\(|clone\\(" src
wc -l src/*.rs
```

Expected: no unfinished-work markers; any remaining `unwrap` or `expect` appears only in tests; large files are split only if a clear responsibility boundary is present.

- [ ] **Step 5: Commit final cleanup**

```bash
git add Cargo.toml src ai.gateway.plist gateway-launch.sh README.md
git commit -m "chore: verify rust gateway"
```

## Self-Review Checklist

- Spec coverage: tasks cover bot mode, run mode, config, sessions, Telegram, Codex, commands, status, launch files, README, and verification.
- Placeholder scan: the plan contains no placeholder markers and no unnamed future work.
- Type consistency: `RunArgs`, `Config`, `SessionStore`, `CodexConfig`, `TelegramClient`, and `ChatSession` are introduced before later tasks reference them.
- Scope: external cron/launchd execution is implemented through `gateway run`; no embedded scheduler or schedule-management commands are added.
