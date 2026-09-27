#!/bin/bash
source "$(dirname "$0")/lib.sh"
LOG="$HOME/Library/Logs/$BUNDLE_ID/moliwhisper.log"
touch "$LOG"
exec tail -n 100 -F "$LOG"
