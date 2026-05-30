# Gateway

Lean Rust Telegram-to-Codex gateway.

## Build

```zsh
cargo test
cargo build --release
```

## Bot

`gateway bot` long-polls Telegram, accepts allowlisted text messages, runs
`codex exec`, and replies with the final answer.

Required secret:

```zsh
export TELEGRAM_BOT_TOKEN=...
export GATEWAY_ALLOWED_IDS=<telegram_chat_id>
```

For launchd, put those exports in `$HOME/$XDG_CONFIG_HOME/gateway/env`, or set
`GATEWAY_ENV_FILE` to another readable env file before running `launch`.
The process also expects `HOME`, `PATH`, `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`,
`XDG_DATA_HOME`, and `XDG_STATE_HOME`.

Useful overrides:

```zsh
export GATEWAY_CODEX_MODEL=gpt-5.5
export GATEWAY_STATE_DIR="$XDG_STATE_HOME/gateway"
```

## Cron or launchd jobs

Run a one-shot Codex prompt without touching the chat bot process:

```zsh
gateway run --job daily --prompt "Summarize the current system state"
```

Send the result to Telegram:

```zsh
gateway run --job daily --prompt-file /path/to/prompt.txt --telegram-chat <telegram_chat_id>
```

## LaunchAgent

The LaunchAgent is repo-rooted without assuming a fixed clone path. The plist
resolves the loaded plist path, runs `launch` next to it, and `launch` runs
`target/release/gateway` from the same repo checkout. Build from the repo root
before starting it.

```zsh
repo_root="$(pwd -P)"
cargo build --release
mkdir -p "$HOME/Library/LaunchAgents"
ln -sfn "$repo_root/ai.gateway.plist" "$HOME/Library/LaunchAgents/ai.gateway.plist"
launchctl bootout "gui/$(id -u)/ai.gateway" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/ai.gateway.plist"
launchctl kickstart -k "gui/$(id -u)/ai.gateway"
```
