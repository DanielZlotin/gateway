#!/bin/zsh
set -euo pipefail

source /Users/example/$XDG_CONFIG_HOME/gateway/env

: "${TELEGRAM_BOT_TOKEN:?TELEGRAM_BOT_TOKEN is required}"

exec env -i \
  HOME=/Users/example \
  TELEGRAM_BOT_TOKEN="$TELEGRAM_BOT_TOKEN" \
  /Users/example/.local/bin/gateway bot
