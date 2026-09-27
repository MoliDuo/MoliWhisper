#!/bin/bash
# Kill running MoliWhisper instance
if pkill -x "MoliWhisper" 2>/dev/null; then
  echo "🛑 MoliWhisper stopped"
else
  echo "ℹ️  MoliWhisper is not running"
fi
