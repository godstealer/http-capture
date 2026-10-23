# Local tun2proxy changes

This directory is based on tun2proxy 0.7.21 (MIT license; see `LICENSE`).
Upstream: https://github.com/tun2proxy/tun2proxy

HTTP Capture's Android VPN sends TCP through its local HTTP capture proxy. The
upstream HTTP proxy handler cannot relay UDP. `direct_udp_for_http` leaves the
default behavior unchanged, but when enabled it routes non-DNS UDP directly
using the upstream no-proxy handler. Virtual-DNS destinations are resolved to
real addresses before the direct UDP socket is opened. The Android JNI entry
enables this flag. These UDP packets are connectivity pass-through, not HTTP
capture records.

The Windows-only Wintun build-script step is also guarded by the Cargo target
OS so that Windows-hosted Android cross-builds do not query Wintun metadata.

Virtual DNS now answers only the queried address family. With Android's
current IPv4 virtual pool, AAAA and HTTPS queries receive a valid empty DNS
answer instead of an unrelated A record; clients can then query A normally.
When IPv6 is enabled, a separate configurable virtual IPv6 pool supplies AAAA
answers and maps both address families back to the original domain. Android
uses `fd00:198:19::/96` for synthetic destinations, separate from its own
`fd00:198:18::2` tunnel address.
