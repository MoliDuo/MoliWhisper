#!/bin/bash
# Run the live ASR probe, e.g.:
#   scripts/asr-probe.sh --wav crates/moli-core/fixtures/zh_short.wav --repeat 20
source "$(dirname "$0")/lib.sh"
exec cargo run --quiet --release -p moli-core --example asr_probe -- "$@"
