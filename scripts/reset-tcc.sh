#!/bin/bash
# Forget the Accessibility, Input Monitoring and microphone grants, to test onboarding.
source "$(dirname "$0")/lib.sh"
for service in Accessibility ListenEvent Microphone; do
  tccutil reset "$service" "$BUNDLE_ID" || true
done
