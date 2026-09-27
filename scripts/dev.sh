#!/bin/bash
# UI work only: `tauri dev` runs the bare binary, so macOS attributes
# Accessibility and microphone to the terminal, not to MoliWhisper.
source "$(dirname "$0")/lib.sh"
pnpm install --frozen-lockfile
exec pnpm tauri dev "$@"
