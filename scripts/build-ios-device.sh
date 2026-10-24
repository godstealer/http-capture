#!/usr/bin/env bash
set -euo pipefail

team_id=${1:-}
device_id=${2:-}
if [[ -z "$team_id" ]]; then
  echo "Usage: bash scripts/build-ios-device.sh TEAM_ID [DEVICE_UDID]" >&2
  exit 2
fi
if [[ $(uname -s) != Darwin ]]; then
  echo "Signed iPhone builds require macOS with Xcode." >&2
  exit 2
fi

for tool in xcodegen xcodebuild codesign cargo rustup python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing required tool: $tool" >&2
    exit 2
  fi
done
if [[ -n "$device_id" ]] && ! xcrun devicectl help >/dev/null 2>&1; then
  echo "Installing to a device requires Xcode's devicectl." >&2
  exit 2
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
ios_root="$repo_root/apps/mobile/ios"
derived_data="$ios_root/build/DeviceDerivedData"

(
  cd "$ios_root"
  xcodegen generate
)

xcodebuild \
  -project "$ios_root/HTTPCaptureMobile.xcodeproj" \
  -scheme HTTPCaptureMobile \
  -configuration Debug \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  -derivedDataPath "$derived_data" \
  -allowProvisioningUpdates \
  DEVELOPMENT_TEAM="$team_id" \
  CODE_SIGN_STYLE=Automatic \
  CODE_SIGNING_ALLOWED=YES \
  build

app="$derived_data/Build/Products/Debug-iphoneos/HTTPCaptureMobile.app"
extension="$app/PlugIns/PacketTunnel.appex"
test -d "$app"
test -d "$extension"
codesign --verify --strict --verbose=2 "$extension"
codesign --verify --strict --verbose=2 "$app"
entitlements_dir=$(mktemp -d "$ios_root/build/signing-check.XXXXXX")
trap 'rm -f "$entitlements_dir/app.plist" "$entitlements_dir/extension.plist"; rmdir "$entitlements_dir"' EXIT
codesign -d --entitlements :- "$app" > "$entitlements_dir/app.plist"
codesign -d --entitlements :- "$extension" > "$entitlements_dir/extension.plist"
python3 - "$entitlements_dir/app.plist" "$entitlements_dir/extension.plist" <<'PY'
import plistlib
import sys

for path in sys.argv[1:]:
    with open(path, "rb") as file:
        entitlements = plistlib.load(file)
    if "group.dev.httpcapture.mobile.ios" not in entitlements.get(
        "com.apple.security.application-groups", []
    ):
        raise SystemExit(f"Signed App Group entitlement is missing: {path}")
    if "packet-tunnel-provider" not in entitlements.get(
        "com.apple.developer.networking.networkextension", []
    ):
        raise SystemExit(f"Signed Packet Tunnel entitlement is missing: {path}")
PY

echo "Signed iPhone app: $app"
if [[ -n "$device_id" ]]; then
  xcrun devicectl device install app --device "$device_id" "$app"
  echo "Installed to $device_id. Open HTTP Capture on the iPhone and grant VPN permission."
else
  echo "To install, pass the iPhone UDID as the second argument or open the Xcode project and run on the device."
fi
