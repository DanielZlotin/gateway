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
