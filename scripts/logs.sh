#!/bin/bash
# Tail logs from running MoliWhisper process via unified logging
# Usage: ./scripts/logs.sh [filter]
# Examples:
#   ./scripts/logs.sh              # all app logs
#   ./scripts/logs.sh HotkeyManager  # only hotkey logs
#   ./scripts/logs.sh Overlay        # only overlay logs

cd "$(dirname "$0")/.."

FILTER="${1:-}"

if pgrep -x "MoliWhisper" > /dev/null 2>&1; then
  PID=$(pgrep -x "MoliWhisper")
  echo "📋 Tailing logs for MoliWhisper (PID: $PID)"
  if [ -n "$FILTER" ]; then
    echo "   Filter: $FILTER"
  fi
  echo "   Press Ctrl+C to stop."
  echo "---"

  if [ -n "$FILTER" ]; then
    log stream --process "$PID" --style compact 2>/dev/null | grep --line-buffered "$FILTER"
  else
    log stream --process "$PID" --style compact 2>/dev/null
  fi
else
  echo "⚠️  MoliWhisper is not running."
  echo "   Start it with: ./scripts/run.sh or ./scripts/dev.sh"
fi
