# 🌉 Gateway

⚡ Lean Rust Telegram-to-Codex gateway.

1. 🤖 `gateway` or `gateway bot`: run the Telegram bot for allowed chats.
2. 🕰️ `gateway run`: execute one fresh Codex prompt from automation.

## 🚀 Setup

Requires macOS with a logged-in desktop session. Install Homebrew if required
tools are missing, and configure the environment below before running setup
from this checkout:

```zsh
./setup
```

`setup` installs missing tools through Homebrew, refreshes Voicebox, downloads
the Whisper model if missing, and builds `target/release/gateway`. It registers
the bot and heartbeat LaunchAgents and starts the bot. Keep this checkout in
place: both agents run from it.

Use `./target/release/gateway` directly, or add this checkout's `target/release`
directory to `PATH` to use the `gateway` commands below. Setup does not install
a separate CLI executable into `PATH`.

For local checks:

```zsh
cargo build --release
cargo test
```

The test suite uses `whisper-cli`, `ffmpeg`, and the large-v3-turbo model installed
by setup. The real transcription test uses bundled audio in `tests/fixtures/speech.ogg`.

## 🌱 Environment

🔐 Required:

```zsh
export GATEWAY_TELEGRAM_TOKEN=...
export GATEWAY_TELEGRAM_CHAT_ID=123456789
```

Replace the example values with a real bot token and positive numeric private
chat ID; usernames and group IDs are rejected. One token can serve multiple
comma-separated chat IDs. For multiple bots, use equal numbers of comma-separated
tokens and chat IDs in matching positions.

The LaunchAgents start a login zsh shell. Export these values and the required
tool paths from your login-shell configuration; exports made only in the setup
terminal are not saved in the agent plists.

⚙️ Optional:

1. 📁 `GATEWAY_CODEX_WORKDIR`: Codex working directory; defaults to `$XDG_CONFIG_HOME`.
2. 🟣 `ANTHROPIC_API_KEY`: required for `claude` model slots.
3. 🌐 `OPENROUTER_API_KEY`: required for `openrouter` model slots.
4. 🔊 `ELEVENLABS_API_KEY`: required when `tts.provider` is `elevenlabs`.
5. 🗂️ `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`: override XDG paths.
   Export `XDG_CONFIG_HOME` explicitly for update and heartbeat runs.

📁 Paths:

1. ⚙️ Config: `$XDG_CONFIG_HOME/gateway/config.json`
2. 💾 State: `$XDG_STATE_HOME/gateway`
3. 📜 Events: `$XDG_STATE_HOME/gateway/logs/gateway.log`
4. 🫀 Heartbeat state: `$XDG_STATE_HOME/gateway/heartbeat.json`
5. 🚀 Bot LaunchAgent: `$HOME/Library/LaunchAgents/ai.gateway.plist`
6. 🫀 Heartbeat LaunchAgent: `$HOME/Library/LaunchAgents/ai.gateway.heartbeat.plist`

📚 Runtime context files live under `$XDG_CONFIG_HOME/gateway/`:

1. 🧭 Always-loaded for Gateway-spawned Codex conversations, in order:
   1. `AGENTS.md`: gateway operating rules, context-loading policy, safety,
      and ownership.
   2. `IDENTITY.md`: assistant identity.
   3. `USER.md`: user preferences and shorthands.
   4. `TOOLS.md`: local environment and tool facts.
   5. `MEMORY.md`: durable facts that do not belong elsewhere.
2. 🫀 Heartbeat-only:
   1. `HEARTBEAT.md`: used only as the `gateway heartbeat` prompt file.
3. 🔄 Creation and refresh:
   1. `gateway run`, `gateway bot`, `gateway heartbeat`, and `gateway status`
      create missing files.
   2. Missing core files start with title/scope headers; missing
      `HEARTBEAT.md` starts with the default heartbeat prompt.
   3. Existing files refresh only title/scope headers, and user
      content below those headers is preserved.
4. 🧠 Loading:
   1. Every Gateway-spawned Codex conversation loads the five core files
      through Codex developer instructions.
   2. `HEARTBEAT.md` is not part of the always-loaded core context.
   3. Manual Codex sessions outside Gateway do not use these Gateway files
      automatically.
   4. Gateway `AGENTS.md` here means runtime context at
      `$XDG_CONFIG_HOME/gateway/AGENTS.md`, not project-local/manual Codex
      `AGENTS.md` auto-discovery.
5. ✍️ Editing:
   1. Gateway-spawned Codex sessions may update these files as long-term context
      when asked or when storing standing instructions.
   2. Each file's `Scope` defines its responsibility; writable targets live
      under `$XDG_CONFIG_HOME/gateway/`.

## ⚙️ Config

Gateway reads `$XDG_CONFIG_HOME/gateway/config.json`; if missing, it creates:

```json
{
  "models": [
    { "provider": "codex" },
    { "provider": "claude", "model": "claude-opus-4-8" },
    { "provider": "openrouter", "model": "openai/gpt-5.5" }
  ],
  "heartbeat": "1d",
  "timeout": "1h"
}
```

📋 Notes:

1. 🧱 Unknown config fields are rejected.
2. 🤖 `models` must include at least one item. Omit `model` (or use an empty string) for Codex to inherit its configured model on new and resumed runs. Explicit Codex models remain overrides; Claude and OpenRouter require non-empty models and run through Codex, not the Claude CLI. Gateway does not copy the resolved model into its config or override inherited reasoning effort or service tier.
3. 🔌 `models[].provider` must be `codex`, `claude`, or `openrouter`.
4. 🧠 The first model is the default. Session naming and Git summaries use it with `low` reasoning; normal runs inherit reasoning. Model roles have been removed: delete existing `role` fields from your config.
5. ⏱️ `timeout` is the Codex/job timeout as a positive whole-number duration using `m` (minutes), `h` (hours), or `d` (days), such as `30m`, `1h`, or `1d`; it defaults to `"1h"`. Replace the removed `timeout_mins` field with `timeout`.
6. 🫀 `heartbeat` defaults to `1d`; use positive `m`, `h`, or `d` durations like `15m`, `3h`, or `1d`.
7. 🕰️ Heartbeat scheduling is anchored to local wall-clock boundaries. For example, `3h` runs at `00:00`, `03:00`, `06:00`, `09:00`, `12:00`, `15:00`, `18:00`, and `21:00`.
8. 🔊 Optional `tts` tries ElevenLabs before local Voicebox. Add this field to
   your existing config alongside `models`:

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

`tts.provider` must be `elevenlabs`; `tts.model` and `tts.voice` are required
non-empty strings; `tts.speed` is optional and must be positive.

## 🧰 CLI

```zsh
gateway
gateway bot
gateway heartbeat
gateway list
gateway logs [lines]
gateway status
gateway update
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
3. 🤖 `--model NAME` overrides the model name using the first configured model's provider.
4. 📤 Final text is printed to stdout; non-empty, non-`OK` text also goes to
   Telegram.
5. 🎯 Without `--chat`, Telegram output goes to the first configured private
   chat ID; with `--chat ID`, it goes only to that configured ID.

🧭 `gateway logs [lines]` defaults to `10` lines and caps at `200`.
It tails the canonical event log, including bot, heartbeat, and update events.

📚 `gateway list [--chat ID]` prints saved sessions for a configured chat.

📊 `gateway status [--chat ID]` prints Codex, gateway, and system status for a
configured chat. CLI and Telegram status distinguish the active model, configured
selection, and last-used model. The active model comes from the running Codex
startup header; it is unknown until that metadata arrives and shows idle after
execution ends. This reports the CLI-selected model, not backend routing.

🫀 `gateway heartbeat` checks whether scheduled work is due; the heartbeat
LaunchAgent invokes it every 60 seconds. When due, it runs the update flow below,
then executes `$XDG_CONFIG_HOME/gateway/HEARTBEAT.md` in a fresh session. An update
failure or an already-running update skips the prompt. Telegram `/heartbeat`
forces a run immediately, including the update step.

📦 `gateway update` runs inline: it pulls this repository and `$XDG_CONFIG_HOME`
(when it is a Git checkout), updates and upgrades Homebrew packages, runs Homebrew
cleanup, writes `$XDG_CONFIG_HOME/homebrew/Brewfile`, then runs `./setup`.
Telegram `/update` starts the same flow in a background job.

🧹 `gateway uninstall` stops both LaunchAgents and removes their plists;
the checkout, configuration, and state remain on disk.

## 🤖 Telegram Bot

Allowed private chats can send text, captions, photos, documents, and voice
messages as Codex prompts.
Sessions are kept separately per chat, and commands are case-insensitive.

```text
🔊 /voice [on|off] - toggle spoken audio replies
📦 /update - update gateway, tools, and setup
✨ /new - start a fresh Codex session
📊 /status - show Codex, gateway, and system status
📚 /list - list saved sessions
↩️ /resume [SESSION_OR_NAME|index] - list or resume a saved session
🏷️ /rename [NAME] - rename the current session
🧠 /model [index] - choose a configured provider/model
🫀 /heartbeat - run heartbeat and print result
📜 /log [lines] - send recent gateway logs
🛑 /stop - cancel this chat's Codex work
```

📋 Notes:

1. 🧠 `/model` (or `/models`) shows the active model, reasoning effort, timeout,
   and selection buttons; `/model 0`, `/model 1`, etc. select by index.
2. ↩️ `/resume` and `/resume 0` list sessions; `/resume 1` steps back one
   saved session; names, full session IDs, and first 8 characters also match.
3. 🏷️ `/rename` without a name asks Codex to create one.
4. 🫧 Bot prompts stream progress and split long final answers.
5. 📎 Photos and image documents are attached; other documents become file paths.
6. 🎙️ Voice messages are transcribed locally with `whisper-cli` (Homebrew `whisper-cpp`)
   using unquantized `large-v3-turbo` and English (`en`). Setup downloads the model
   (~1.6 GB) to `$XDG_DATA_HOME/gateway/whisper/ggml-large-v3-turbo.bin` once. FFmpeg
   converts Telegram audio to 16 kHz mono PCM WAV; conversion and transcription
   share a 120-second timeout. Temporary conversion files are removed afterward.
7. 🔊 `/voice` toggles spoken replies for the current session. `/new`, `/resume`,
   and model changes disable voice mode.
   Voice replies try `tts`, then local Voicebox.
