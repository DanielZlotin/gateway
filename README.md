# 🌉 Gateway

⚡ Lean Rust Telegram-to-Codex gateway.

1. 🤖 `gateway` or `gateway bot`: run the Telegram bot for allowed chats.
2. 🕰️ `gateway run`: execute one fresh Codex prompt from automation.

## 🚀 Setup

```zsh
./setup
```

`setup` installs local tools, refreshes Voicebox, builds the release binary,
installs the LaunchAgent, and restarts the bot.

For local checks:

```zsh
cargo test
cargo build --release
```

## 🌱 Environment

🔐 Required:

```zsh
export GATEWAY_TELEGRAM_TOKEN=...
export GATEWAY_TELEGRAM_CHAT_ID=123456789
```

For multiple bots, use comma-separated token and chat ID values in matching
positions.

⚙️ Optional:

1. 📁 `GATEWAY_CODEX_WORKDIR`: Codex working directory.
2. 🟣 `ANTHROPIC_API_KEY`: required for `claude` model slots.
3. 🌐 `OPENROUTER_API_KEY`: required for `openrouter` model slots.
4. 🔊 `ELEVENLABS_API_KEY`: required when `tts.provider` is `elevenlabs`.
5. 🗂️ `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`: override XDG paths.

📁 Paths:

1. ⚙️ Config: `$XDG_CONFIG_HOME/gateway/config.json`
2. 💾 State: `$XDG_STATE_HOME/gateway`
3. 📜 Events: `$XDG_STATE_HOME/gateway/logs/gateway.log`
4. 🫀 Heartbeat state: `$XDG_STATE_HOME/gateway/heartbeat.json`
5. 🚀 LaunchAgent: `$HOME/Library/LaunchAgents/ai.gateway.plist`

## ⚙️ Config

Gateway reads `$XDG_CONFIG_HOME/gateway/config.json`; if missing, it creates:

```json
{
  "models": [
    { "provider": "codex", "model": "gpt-5.5" },
    { "provider": "codex", "model": "gpt-5.4-mini", "role": "light" },
    { "provider": "claude", "model": "claude-opus-4-8" },
    { "provider": "openrouter", "model": "openai/gpt-5.5" }
  ],
  "heartbeat": "1d",
  "timeout_mins": 30
}
```

📋 Notes:

1. 🤖 `provider` must be `codex`, `claude`, or `openrouter`.
2. 🧠 Missing `role` means `default`; `role: "light"` marks the helper model.
3. 🫀 `heartbeat` defaults to `1d`; use `m`, `h`, or `d` durations like `3h`.
4. ⏱️ `timeout_mins` defaults to `30`.
5. 🔊 Optional `tts` tries ElevenLabs before local Voicebox:

```json
{
  "tts": {
    "provider": "elevenlabs",
    "model": "eleven_v3",
    "voice": "cPoqAvGWCPfCfyPMwe4z",
    "speed": 1.5
  }
}
```

`speed` is optional. Invalid, missing, or failing `tts` falls back to local
Voicebox.

## 🧰 CLI

```zsh
gateway
gateway bot
gateway heartbeat
gateway logs [lines]
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
3. 🤖 `--model NAME` overrides the default model.
4. 📤 Final text is printed to stdout; non-empty, non-`OK` text also goes to
   Telegram.
5. 🎯 Without `--chat`, Telegram output goes to the first configured private
   chat ID; with `--chat ID`, it goes only to that configured ID.

🧭 `gateway logs [lines]` defaults to `10` lines and caps at `200`.
It tails the canonical event log, including bot, heartbeat, and update events.

## 🤖 Telegram Bot

Allowed private chats can send text, captions, photos, documents, and voice
messages as Codex prompts.
Sessions are kept separately per chat, and commands are case-insensitive.

```text
🔊 /voice [on|off] - toggle spoken audio replies for the current session
📦 /update - pull latest gateway code, update Brew/Foundry, and run setup
✨ /new - start a fresh Codex session
📚 /list - list saved sessions
↩️ /resume [SESSION_OR_NAME|index] - list or resume a saved session
🏷️ /rename [NAME] - rename the current session
🧠 /model [index] - choose a configured provider/model
📊 /status - show Codex, gateway, and system status
📜 /log [lines] - send recent gateway logs
🔁 /restart - restart the gateway service
🛑 /stop - cancel active and queued Codex work for this chat
```

📋 Notes:

1. 🧠 `/model` lists buttons; `/model 0`, `/model 1`, etc. select by index.
2. ↩️ `/resume` and `/resume 0` list sessions; `/resume 1` steps back one
   saved session; names, full session IDs, and first 8 characters also match.
3. 🏷️ `/rename` without a name asks Codex to create one.
4. 🫧 Bot prompts stream progress and split long final answers.
5. 📎 Photos and image documents are attached; other documents become file paths.
6. 🎙️ Voice messages are transcribed locally with Whisper `large`.
7. 🔊 `/voice` toggles spoken replies for the current session. `/new`, `/resume`,
   `/restart`, and model changes disable voice mode.
   Voice replies try `tts`, then local Voicebox.
