#!/usr/bin/env bash
set -euo pipefail

app=apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk
test_app=apps/mobile/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
probe_app=apps/mobile/android/vpn-probe/build/outputs/apk/debug/vpn-probe-debug.apk
test -f "$app"
test -f "$test_app"
test -f "$probe_app"
adb install -r "$app"
adb install -r "$test_app"
adb install -r "$probe_app"

origin_log="$(mktemp)"
udp_log="$(mktemp)"
python3 -m http.server 18080 --bind 127.0.0.1 >"$origin_log" 2>&1 &
origin_pid=$!
python3 -u scripts/android-udp-echo.py >"$udp_log" 2>&1 &
udp_pid=$!
trap 'kill "$origin_pid" "$udp_pid" 2>/dev/null || true; rm -f "$origin_log" "$udp_log"' EXIT
for _ in $(seq 1 20); do
  if curl --noproxy '*' -s -o /dev/null http://127.0.0.1:18080/; then break; fi
  sleep 1
done
kill -0 "$origin_pid"
for _ in $(seq 1 20); do
  if grep -Fq 'UDP echo ready' "$udp_log"; then break; fi
  kill -0 "$udp_pid"
  sleep 1
done
grep -Fq 'UDP echo ready' "$udp_log"
adb logcat -c

if ! output="$(timeout 150s adb shell am instrument -w \
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
if ! grep -Fq 'GET /vpn-smoke' "$origin_log"; then
  cat "$origin_log"
  echo 'VPN raw TCP request did not reach the origin' >&2
  exit 1
fi
echo 'VPN raw TCP request reached the origin and capture record'

if ! grep -Fq 'GET /vpn-selected-smoke' "$origin_log" ||
   ! grep -Fq 'GET /vpn-excluded-smoke' "$origin_log"; then
  cat "$origin_log"
  echo 'Selected-app VPN test traffic did not reach the origin' >&2
  exit 1
fi
echo 'Selected app was captured and the excluded app reached the origin'

udp_ok=false
for _ in $(seq 1 20); do
  probe_log="$(adb logcat -d -s 'HttpCaptureProbe:I' '*:S')"
  if grep -Fq 'VPN UDP marker received' "$udp_log" &&
     grep -Fq 'raw VPN UDP echo succeeded' <<<"$probe_log"; then
    udp_ok=true
    break
  fi
  sleep 1
done
if [ "$udp_ok" != true ]; then
  cat "$udp_log"
  adb logcat -d -s 'HttpCaptureProbe:V' '*:S' || true
  echo 'VPN raw UDP echo did not complete' >&2
  exit 1
fi
echo 'VPN raw UDP reached the origin and returned unchanged'

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
