#!/usr/bin/env bash
set -euo pipefail

# Keep the original local smoke mandatory before the remote PC/VPN test.
bash scripts/smoke-android.sh
python3 scripts/smoke-android-lan.py \
  --fixture target/debug/examples/android_lan_fixture \
  --summary "${RUNNER_TEMP:?}/android-lan-e2e-summary.json"
