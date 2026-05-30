# Rust Gateway Design

## Goal

Rewrite the Go Telegram-to-Codex gateway from `example/gateway`
as a lean Rust service in this repository while preserving the user-facing
behavior and adding a simple external cron/launchd execution path.

## Source Baseline

The Go gateway is a single-binary service that:

- long-polls Telegram for text messages;
- accepts messages from an allowlist;
- handles `/commands`, `/help`, `/status`, `/log`, `/new`, `/restart`,
  `/model`, `/resume`, `/rename`, and `/list`;
- runs `codex exec` in yolo mode and replies with the final response;
- stores per-chat session state as JSON;
- syncs Telegram bot commands on startup;
- sends startup status to allowlisted chats;
- installs through a macOS user LaunchAgent and a small zsh launcher.

The Rust rewrite keeps those behaviors unless this spec explicitly changes
them.

## Product Shape

The binary exposes two modes:

1. `gateway bot`
   Runs the Telegram long-polling bot. Chat execution is synchronous per job:
   a Telegram prompt runs Codex and the result is sent back to the same chat.
   The bot keeps a bounded queue so it can accept new updates while one job is
   running.

2. `gateway run`
   Runs one Codex prompt for cron/launchd usage. This process is independent of
   `gateway bot`, so scheduled jobs execute asynchronously relative to chat.
   The command reads prompt text from an argument, stdin, or a prompt file. It
   can optionally send the result to a configured Telegram chat.

No embedded scheduler is included. Cron or launchd owns scheduling. This avoids
cron parsing, persistent job registries, and extra background state.

## Configuration

Configuration is intentionally small:

- `TELEGRAM_BOT_TOKEN` remains the only required secret.
- Non-secret defaults match the Go gateway where practical:
  - allowed Telegram ID: `<telegram_chat_id>`;
  - Codex binary: `/opt/homebrew/bin/codex`;
  - Codex workdir: `/Users/example/.config`;
  - Codex home: `/Users/example/.config/codex`;
  - default model: `gpt-5.5`;
  - state dir: `/Users/example/.local/state/gateway`;
  - log file: `/Users/example/.local/share/gateway/logs/gateway.log`;
  - queue depth: `8`;
  - Codex timeout: `45m`.
- Environment variables may override paths, allowlist, model, timeout, and
  queue depth.
- No config file is included in the first implementation. Environment variables
  are the configuration surface.

The rewrite avoids compatibility aliases and fallback behavior unless needed to
preserve the Go gateway's current public behavior.

## Architecture

The code is split by responsibility to keep cyclomatic complexity low:

- `main.rs`: CLI routing, startup, shutdown, and mode selection.
- `config.rs`: default values, environment parsing, and validation.
- `bot.rs`: Telegram poll loop, offset handling, bounded queue, worker thread.
- `commands.rs`: slash command parsing and pure command decisions.
- `telegram.rs`: Telegram DTOs, request encoding, response decoding, and token
  redaction.
- `codex.rs`: Codex args, environment, process execution, timeout handling, and
  JSON event parsing.
- `session.rs`: chat and cron session state load/save/list/resume/rename.
- `status.rs`: status header and optional `fastfetch` snapshot.
- `text.rs`: Telegram message splitting, command args, log tailing, and small
  formatting helpers.

Most branching lives in pure functions that return small enums or structs. I/O
wrappers remain thin.

## State Model

State stays JSON and human-readable:

- chat state is keyed by Telegram chat ID;
- cron state is keyed by a caller-provided job name;
- both use the same session fields:
  - current Codex session ID;
  - model;
  - generation;
  - updated timestamp;
  - saved sessions with ID, optional name, model, and updated timestamp.

The generation check from the Go gateway is preserved so stale Codex responses
do not overwrite a newer `/new` or `/resume` decision.

## Command Behavior

The Telegram command behavior remains:

- `/commands` and `/help` show supported directives;
- `/status` shows model, session label, and fastfetch output when available;
- `/log [lines]` returns recent gateway logs, defaulting to 80 and capping at
  200;
- `/new` clears the current chat session and increments generation;
- `/restart` asks launchctl to restart the gateway service;
- `/model [name]` shows or sets the current model;
- `/resume SESSION_OR_NAME` resumes a saved session by full ID, short ID, or
  name;
- `/rename NAME` names the current session;
- `/list` lists saved sessions.

Unknown slash commands are rejected. Non-command text is sent to Codex.
Prompt and response bodies are not copied into gateway logs.

## Cron Execution

`gateway run` supports the smallest useful surface:

- `--job NAME` selects the independent cron session key;
- `--prompt TEXT`, `--prompt-file PATH`, or stdin provides the prompt;
- `--model MODEL` overrides the default model for this run;
- `--new` starts without resuming the saved cron session;
- `--telegram-chat ID` sends the final output to Telegram;
- stdout always receives the final output for cron logs.

The run mode shares Codex execution, session persistence, message splitting,
and Telegram sending code with bot mode.

## Error Handling

Errors are explicit and boring:

- configuration errors fail startup with a clear message;
- Telegram API errors include method name and redacted token context;
- Codex timeout returns any partial final text plus stderr;
- failed session writes are logged and surfaced to the user when relevant;
- `gateway run` exits non-zero when configuration, prompt loading, Codex, or
  Telegram delivery fails.

## Testing

The implementation targets full practical test coverage without real network or
Codex calls:

- unit tests for config defaults and environment overrides;
- unit tests for CLI routing and `gateway run` prompt source precedence;
- unit tests for command parsing and directive responses;
- unit tests for Codex args, environment, JSON event parsing, and timeout
  result shaping;
- unit tests for session load/save/upsert/find/list/generation behavior using
  temporary directories;
- unit tests for Telegram request encoding, command scope payloads, response
  decoding, and token redaction;
- unit tests for message splitting, status formatting, command args, and log
  tailing;
- focused integration tests with fake Telegram and fake Codex adapters where
  I/O orchestration matters.

Verification commands are `cargo test`, `cargo fmt --check`, and
`cargo clippy --all-targets -- -D warnings`.

## Non-Goals

- No embedded cron scheduler.
- No Telegram commands for creating schedules.
- No database.
- No web server.
- No async runtime unless blocking I/O makes a requirement materially harder.
- No broad compatibility shims beyond the Go gateway's current public behavior.

## Open Decisions Resolved

- Cron execution uses external cron/launchd via `gateway run`.
- Chat mode stays synchronous per Codex job while the bot loop remains
  responsive through a bounded queue and worker thread.
- The Rust rewrite can use small crates when they reduce code and complexity.
  It will avoid a large framework.
