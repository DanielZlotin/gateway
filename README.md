# 🌉 Gateway

⚡ Lean Rust Telegram-to-Codex gateway.

1. 🤖 `gateway` or `gateway bot` long-polls Telegram and answers allowed chats.
2. 🕰️ `gateway run` executes one Codex prompt for cron/scripts and can notify Telegram.

## 🛠️ Build

```zsh
cargo test
cargo build --release
```

## 🌱 Environment

🔐 Required:

```zsh
export GATEWAY_TELEGRAM_TOKEN=...
export GATEWAY_TELEGRAM_CHAT_IDS=123456789,-1001234567890
```

`GATEWAY_TELEGRAM_CHAT_IDS` is comma-separated, trimmed, integer-parsed, sorted,
and deduplicated.

⚙️ Optional:

1. 📁 `GATEWAY_CODEX_WORKDIR`: Codex working directory.
2. 📁 `XDG_CONFIG_HOME`: defaults to `$HOME/.config`.
3. 📁 `XDG_CACHE_HOME`: defaults to `$HOME/.cache`.
4. 📁 `XDG_DATA_HOME`: defaults to `$HOME/.local/share`.
5. 📁 `XDG_STATE_HOME`: defaults to `$HOME/.local/state`.

🔐 Provider API keys:

1. 🟣 `ANTHROPIC_API_KEY`: required when Claude is selected.
2. 🌐 `OPENROUTER_API_KEY`: required when OpenRouter is selected.

📌 Fixed runtime values:

1. 📁 state root: resolved `XDG_STATE_HOME` plus `/gateway`
2. 📜 log file: `logs/gateway.log`
3. ⏱️ Telegram poll timeout: `50` seconds
4. 🚦 bot job queue depth: `8`
5. 🚀 launchd target: `gui/<current uid>/ai.gateway`

## 🚀 Setup

```zsh
./setup
```

`setup` verifies required env, applies XDG defaults, checks PATH tools (`cargo`,
`codex`, `date`, `fastfetch`, `id`, `jq`, `launchctl`, `mkdir`, `mv`, `rm`),
builds release, writes `ai.gateway.plist` with the absolute `launch` path into
`$HOME/Library/LaunchAgents`, then runs launchd `bootout`, `bootstrap`, and
`kickstart`.

`launch` logs to the state log file and execs `target/release/gateway bot`.

## ⚙️ Config

Gateway creates and normalizes `gateway/config.json` under the resolved
`XDG_CONFIG_HOME`:

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

1. 🤖 `models` is an ordered array of `{ "provider", "model" }` entries.
2. ⏱️ `timeout_mins: 0` resets to `30`.
3. ⚠️ Overflowing `timeout_mins` fails config load.
4. 🔌 Each `provider` is `codex`, `claude`, or `openrouter`.
5. 🧠 Default config includes Codex, Claude, and OpenRouter slots; if `models`
   is empty, a Codex slot is inserted.
6. 💾 `/model` selections are session-only and do not rewrite config.

## 💾 State

📁 Files under the state root:

```text
telegram.offset
logs/gateway.log
chats/<chat_id>-main.json
chats/<chat_id>-thread-<thread_id>.json
cron/<job>.json
```

Chat sessions are scoped by chat/thread. Run sessions are scoped by `--job`;
unsafe job characters become `_`. Session state stores current Codex session ID,
model, generation, updated time, and saved sessions. Saved sessions store ID,
optional name, model, and updated time. Generation checks prevent stale Codex
runs from overwriting a newer `/new` or `/resume`.

## 🧰 CLI

```zsh
gateway
gateway bot
gateway logs [lines]
gateway paths
gateway uninstall
gateway run --job daily --prompt "Summarize status"
gateway run --job daily --prompt-file ./prompt.txt
printf '%s\n' "Summarize status" | gateway run --job daily
```

🏃 Run flags:

1. 🧵 `--job NAME`: required session namespace.
2. 💬 `--prompt TEXT`: first prompt source.
3. 📄 `--prompt-file PATH`: second prompt source.
4. ⌨️ stdin: third prompt source.
5. 🤖 `--model NAME`: model override for the run.
6. 🆕 `--new`: reset the run session before execution.

Prompts are trimmed; empty prompts fail. `gateway run` prints final Codex text to
stdout and sends Telegram only when the trimmed result is non-empty and not
exactly `OK` case-insensitively. CLI errors print to stderr and exit `1`.

`gateway paths` prints the resolved config, state, log, session, executable, and
LaunchAgent paths. `gateway logs [lines]` prints recent gateway logs to stdout,
defaults to `10` lines, caps at `200`, and uses the resolved `XDG_STATE_HOME`.
`gateway uninstall` runs launchd `bootout` for `ai.gateway` and removes
`$HOME/Library/LaunchAgents/ai.gateway.plist`.

## 🤖 Bot

🚀 Startup:

1. 📁 Creates state, chat, and cron directories.
2. 🧭 Syncs Telegram commands for default/private/allowed-chat scopes and clears
   broader group/admin scopes.
3. 📊 Sends status to each allowed chat.
4. ⏭️ On first poll, skips existing Telegram backlog and stores the next offset.

💬 Messages:

1. 🚫 Non-allowed chats are ignored.
2. 📝 Text and captions are accepted; blank/non-text input gets `📝 Text messages only.`.
3. ⚡ Commands run immediately.
4. 🚦 Normal prompts enter one Codex worker queue; a full queue gets a queue-full reply.
5. 🫧 A running job sends a thinking message, refreshes Telegram typing, streams
   Codex stdout by editing that message, then delivers the final result.

📬 Final delivery:

1. 📭 Empty final text becomes `📭 Codex finished with no final text.`.
2. 👍 Trimmed `OK` deletes the thinking message and reacts thumbs-up to the request.
3. ✨ Other final text tries Telegram effect `5107584321108051014`, then falls back
   to a normal message.
4. ✂️ Long messages split under `3900` chars, preferring a recent newline.
5. ⚠️ Codex failures are sent as `⚠️ Codex failed:` plus collected error text.

## 🧭 Commands

🔤 Commands are case-insensitive and may include a bot suffix such as
`/status@MyBot`.

```text
🧭 /commands - show supported gateway directives
❔ /help - alias for /commands
📊 /status - show Codex, gateway, and system status
📜 /log [lines] - send recent gateway logs
🆕 /new - start a fresh Codex session
🔄 /restart - restart the gateway service
🤖 /model [index] - choose a configured provider/model
↩️ /resume SESSION_OR_NAME - resume a saved session
🏷️ /rename NAME - rename the current session
💾 /list - list saved sessions
```

📋 Details:

1. 📊 `/status` sends model/session, Codex usage, then fastfetch.
2. 📜 `/log` defaults to `10` lines and caps at `200`.
3. 🆕 `/new` clears current session ID and increments generation.
4. 🔄 `/restart` spawns `/bin/launchctl kickstart -k <target>`.
5. 🤖 `/model` with no argument sends inline buttons for every configured
   provider/model slot.
6. 🔢 `/model 0`, `/model 1`, etc. select by zero-based `models` array index.
7. 🧠 `/new` and `/resume` return the chat to `models[0]`.
8. ↩️ `/resume` matches full ID, first 8 chars, or saved name.
9. 🏷️ `/rename` requires a current session.
10. 💾 `/list` marks the current saved session with `⭐`.
11. ❓ Unknown commands return the defined directive list.

## 🧠 Codex

Gateway injects `src/SYSTEM.md` as `developer_instructions`, enables live search,
passes prompts on stdin, and lets Codex inherit Gateway's environment. All
providers run through Codex.

`codex` provider slots use the existing logged-in Codex subscription auth.
`claude` provider slots start a loopback Anthropic adapter, then run Codex with
provider overrides equivalent to:

```toml
model_provider = "anthropic-gateway"

[model_providers.anthropic-gateway]
name = "Anthropic Gateway"
base_url = "http://127.0.0.1:<ephemeral>/v1"
env_key = "ANTHROPIC_API_KEY"
wire_api = "responses"
```

The adapter translates Codex Responses API requests to Anthropic's
OpenAI-compatible `/v1/chat/completions` endpoint and returns Responses-style
SSE events to Codex. It is local to one Codex run and does not use OpenRouter.

`openrouter` provider slots run Codex with OpenRouter provider overrides
equivalent to:

```toml
model_provider = "openrouter"

[model_providers.openrouter]
name = "openrouter"
base_url = "https://openrouter.ai/api/v1"
env_key = "OPENROUTER_API_KEY"
wire_api = "responses"
```

Gateway does not write Codex credentials or edit Codex `config.toml`; Codex reads
the selected provider API key from the inherited environment. Model names come
from `models` in `gateway/config.json`, not from provider model env overrides.

🆕 New sessions:

```text
codex --search exec --color never -c developer_instructions=<SYSTEM.md> --cd <workdir> --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m <model> --output-last-message <tmpfile> -
```

↩️ Resumed sessions:

```text
codex --search exec resume -c developer_instructions=<SYSTEM.md> --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m <model> --output-last-message <tmpfile> <session_id> -
```

📤 Output rules:

1. 📬 Final text comes from `--output-last-message`, falling back to trimmed stdout.
2. 🫧 Raw stdout chunks drive streaming previews.
3. 🧵 Session IDs come from stderr lines prefixed `session id:`.
4. ⏱️ Timeouts kill Codex and include output-file text, parsed stdout
   `agent_message` text, and stderr when available.
5. 🔐 `src/SYSTEM.md` requires concise Telegram-safe replies and blocks secrets or
   private data from being sent.

## 📲 Telegram

1. 📥 `getUpdates` requests only `message` updates.
2. 📝 Messages use Markdown parse mode and disable web previews.
3. 🔁 Markdown parse failures retry as plain text.
4. 🔐 Request errors redact the bot token.
5. 🛠️ Gateway can send, edit, delete, type, set reactions, and send final effects.
6. 🌐 Bot commands are registered for default language, `en`, and `he`.

## 📊 Status

📡 Startup status and `/status` include:

1. 🌉 Gateway header: model and current session label, or `none`.
2. 🧠 Codex usage: reads `codex/auth.json` under the resolved `XDG_CONFIG_HOME`, calls
   `https://chatgpt.com/backend-api/wham/usage`, and reports primary/secondary
   usage percentage left without exposing tokens.
3. 🖥️ Fastfetch: runs `fastfetch --config - --pipe` with bundled config on stdin,
   kills it after `5` seconds, and sends partial output with a timeout note.

## 🚀 LaunchAgent

`setup` installs `ai.gateway.plist` with an absolute path to the repo's `launch`
script. The LaunchAgent uses `RunAtLoad`, `KeepAlive`, `ThrottleInterval = 1`,
and `Umask = 63`. The install location remains
`$HOME/Library/LaunchAgents` because that is macOS launchd convention; Gateway
config, state, cache, and data paths use explicit XDG env vars or their standard
home-relative defaults.
