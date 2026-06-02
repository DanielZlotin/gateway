# 🌉 Gateway

⚡ Lean Rust Telegram-to-Codex gateway.

1. 🤖 `gateway` or `gateway bot`: run the Telegram bot for allowed chats.
2. 🕰️ `gateway run`: execute one fresh Codex prompt from automation.

## 🛠️ Build

```zsh
cargo test
cargo build --release
```

## 🌱 Environment

🔐 Required:

```zsh
export GATEWAY_TELEGRAM_TOKEN=...
export GATEWAY_TELEGRAM_CHAT_IDS=123456789
```

⚙️ Optional:

1. 📁 `GATEWAY_CODEX_WORKDIR`: Codex working directory.
2. 🟣 `ANTHROPIC_API_KEY`: required for `claude` model slots.
3. 🌐 `OPENROUTER_API_KEY`: required for `openrouter` model slots.
4. 🗂️ `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`:
   override the standard `$HOME/.config`, `$HOME/.cache`,
   `$HOME/.local/share`, and `$HOME/.local/state` defaults.

📁 Paths:

1. ⚙️ Config: `$XDG_CONFIG_HOME/gateway/config.json`
2. 💾 State: `$XDG_STATE_HOME/gateway`
3. 📜 Logs: `$XDG_STATE_HOME/gateway/logs/gateway.log`
4. 🚀 LaunchAgent: `$HOME/Library/LaunchAgents/ai.gateway.plist`

## 🚀 Setup

```zsh
./setup
```

`setup` builds the release binary, installs the macOS LaunchAgent, and starts or
restarts the bot.

## ⚙️ Config

Gateway reads model slots and timeout settings from
`$XDG_CONFIG_HOME/gateway/config.json`. If the file is missing, Gateway creates
it with these defaults:

```json
{
  "models": [
    {
      "provider": "codex",
      "model": "gpt-5.5"
    },
    {
      "provider": "claude",
      "model": "claude-opus-4-8"
    },
    {
      "provider": "openrouter",
      "model": "openai/gpt-5.5"
    }
  ],
  "timeout_mins": 30
}
```

📋 Rules:

1. 🤖 `provider` must be `codex`, `claude`, or `openrouter`.
2. 🧠 The first model slot is the default for new sessions and `gateway run`.
3. ⏱️ `timeout_mins` sets the per-prompt Codex timeout.
4. 💾 `/model` changes the current chat session only; it does not edit config.
5. 🧱 Existing config files must include `models`; `timeout_mins` defaults to
   `30` when omitted.

## 🧰 CLI

```zsh
gateway
gateway bot
gateway logs [lines]
gateway config
gateway uninstall
gateway version
gateway run --prompt "Summarize status"
gateway run --chat 123456789 --prompt "Summarize status"
gateway run --prompt-file ./prompt.txt
printf '%s\n' "Summarize status" | gateway run
```

🏃 `gateway run`:

1. 💬 Prompt input comes from `--prompt`, then `--prompt-file`, then stdin.
2. 🆕 Each invocation starts a fresh Codex session.
3. 🤖 `--model NAME` overrides the default model for that run.
4. 📤 Final text is always printed to stdout.
5. 📬 Non-empty, non-`OK` final text is sent to one Telegram chat.
6. 🎯 Without `--chat`, Telegram output goes to the first configured private
   chat ID.
7. 💬 With `--chat ID`, Telegram output goes only to that ID, and the ID must
   already be listed in `GATEWAY_TELEGRAM_CHAT_IDS`.

🧭 Other commands:

1. 📜 `gateway logs [lines]` prints recent logs; default `10`, max `200`.
2. ⚙️ `gateway config` prints loaded gateway config with secrets redacted.
3. 🧹 `gateway uninstall` stops the LaunchAgent and removes its plist.
4. 🧾 `gateway version` prints the running binary version.

## 🤖 Telegram Bot

Allowed private chats can send text messages or captions as Codex prompts.
Sessions are kept separately per chat, and commands are case-insensitive.

```text
❔ /help - show supported gateway directives
📊 /status - show Codex, gateway, and system status
⚙️ /config - show loaded gateway config with secrets redacted
📜 /log [lines] - send recent gateway logs
🆕 /new - start a fresh Codex session
🔄 /restart - restart the gateway service
🤖 /model [index] - choose a configured provider/model
↩️ /resume [SESSION_OR_NAME|index] - list or resume a saved session
🏷️ /rename [NAME] - rename the current session
💾 /list - list saved sessions
```

📋 Command notes:

1. 🤖 `/model` with no argument shows model buttons; `/model 0`, `/model 1`,
   etc. select by config index.
2. 🆕 `/new` starts a fresh session using the default model slot.
3. ↩️ `/resume` and `/resume 0` list sessions; `/resume 1` steps back one
   saved session; non-numeric values still match a full session ID, first 8
   characters, or saved name.
4. 🏷️ `/rename NAME` names the current session; `/rename` asks Codex for a
   concise name automatically.
5. 📜 `/log` defaults to `10` lines and caps at `200`.

## 📬 Results

1. 🫧 Bot prompts stream progress in Telegram and then send the final answer.
2. 👍 A final `OK` is treated as a quiet success.
3. ✂️ Long final answers are split into Telegram-sized messages.
4. ⚠️ Codex or provider failures are returned to the requesting chat.
