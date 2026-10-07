# HTTP Capture TUN helper

Optional GPL-3.0-or-later helper linking sing-box 1.14.0. See LICENSE and ../../docs/tun.md.

Protocol: first stdin line is JSON configuration; stdout emits `{"ready":true}` only after Start succeeds. Closing stdin or sending another line requests shutdown and Close. `--check` only validates and constructs the engine without starting TUN. `--version` prints protocol and engine versions. Build requires Go 1.25.5+; go.mod and go.sum pin dependencies.

This process must run with platform network privileges. The parent does not install certificates, elevate automatically, or configure the firewall.
