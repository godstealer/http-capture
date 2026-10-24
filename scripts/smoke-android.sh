#!/usr/bin/env bash
set -euo pipefail

app=apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk
test_app=apps/mobile/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
test -f "$app"
test -f "$test_app"
adb install -r "$app"
adb install -r "$test_app"

if ! output="$(timeout 120s adb shell am instrument -w \
  dev.httpcapture.mobile.test/dev.httpcapture.mobile.CaptureSmokeInstrumentation 2>&1)"; then
  printf '%s\n' "$output"
  adb logcat -d -t 200 '*:E' || true
  exit 1
fi
printf '%s\n' "$output"
if ! grep -Fq 'smokeStatus=ok' <<<"$output" ||
   ! grep -Fq 'INSTRUMENTATION_CODE: -1' <<<"$output"; then
  adb logcat -d -t 200 '*:E' || true
  exit 1
fi

adb shell am start -W -n dev.httpcapture.mobile/.MainActivity
sleep 3
for _ in $(seq 1 20); do
  if adb shell pidof dev.httpcapture.mobile >/dev/null; then
    echo 'Android app launch succeeded'
    exit 0
  fi
  sleep 1
done
adb logcat -d -t 200 '*:E' || true
echo 'Android app process did not stay running' >&2
exit 1
