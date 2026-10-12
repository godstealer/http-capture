#!/usr/bin/env bash
# Never use the caller's session bus or dconf database for mutation tests.
set -euo pipefail
config_dir=$(mktemp -d /tmp/http-capture-gnome-XXXXXXXX)
trap 'rm -rf -- "$config_dir"' EXIT
export XDG_CONFIG_HOME="$config_dir"
export XDG_CACHE_HOME="$config_dir/cache"
export XDG_STATE_HOME="$config_dir/state"
export XDG_RUNTIME_DIR="$config_dir/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
printf 'user-db:user\n' > "$config_dir/profile"
export DCONF_PROFILE="$config_dir/profile"
export XDG_CURRENT_DESKTOP=GNOME GSETTINGS_BACKEND=dconf HTTP_CAPTURE_GNOME_TEST=1
unset DBUS_SESSION_BUS_ADDRESS
# All activated dconf services inherit the disposable configuration paths.
dbus-run-session -- cargo test --locked -p capture-core --lib gnome_isolated_session_roundtrip -- --ignored
