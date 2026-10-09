# HTTP/2 and HTTP/3 (experimental)

Auto is the default engine for newly composed requests: HTTPS advertises h2 and http/1.1 in a single TLS handshake; HTTP uses HTTP/1.1. ALPN chooses the upstream protocol before any HTTP request is sent. A failed request is never retried by changing protocols. The native engine explicitly selects HTTP/1.1. Select `h2` or `h3`
under Settings to send using that protocol. These engines validate the
server certificate, require HTTPS and compatible per-request TLS settings (H3 requires TLS 1.3), and fail rather
than silently falling back to HTTP/1.1. H3 sends QUIC over UDP directly to the
URL's port; it does not discover alternative ports through Alt-Svc.

The CONNECT proxy negotiates `h2` or `http/1.1` using ALPN. H2 connections
support concurrent streams and use Auto for the independent upstream TLS negotiation: h2 when selected, otherwise HTTP/1.1. The captured client protocol remains HTTP/2; the response records the actual upstream protocol. Manually selecting the h2 engine still requires h2 and never silently downgrades. H1 capture forwards with the native H1 engine; upstream h2 support does not upgrade a captured H1 request. Connections
currently have a 60-second lifetime. Bodies are limited to 8 MiB; trailers,
extended CONNECT and server push are not supported. WebSocket uses HTTP/1 Upgrade as described below.

## SSE (2026-10-09)

Native Auto/H1/H2 sends and H1/H2 capture now stream `text/event-stream` response headers and entity bytes through bounded channels. HTTP/1 chunked framing is removed incrementally, including events inside an unfinished chunk; downstream H1 is close-delimited, downstream H2 uses DATA frames. Content-Encoding bytes remain unchanged. Snapshots are persisted at up to four updates per second and displayed by the existing UI refresh mechanism.

Manual sends use the existing cancel button; captured SSE has a Stop SSE button. Cancellation and stream errors retain received body bytes. Sessions have an explicit 300-second / 8-MiB bound; regular requests retain the 30-second timeout. No automatic reconnect or replay is performed. Response scripts and matching response interception rules reject streaming before forwarding its headers; request-side processing remains supported.

The SSE viewer shows blank-line-terminated events, event type, inherited ID, multi-line data and retry. Heartbeats and partial events remain visible in raw text. Compressed streams are forwarded live, but incremental event viewing of unfinished compressed bodies is not guaranteed (use identity encoding for live event inspection). H3, wreq and httpcloak still use buffered responses.

## WebSocket capture (2026-10-09)

HTTP/1 Upgrade WS and CONNECT + TLS + HTTP/1 Upgrade WSS can now be captured. The native TLS client validates upstream certificates; clients must trust the capture CA for WSS. HTTP and SOCKS upstream routing uses the existing connection layer. Upgrade validates the request key/version, server accept key and selected subprotocol. Non-101 upstream responses are recorded as handshake errors; they are not upgraded.

Bidirectional frame bytes are relayed unchanged, including masking, fragmentation and negotiated permessage-deflate. The persisted WebSocket frame list contains direction, elapsed time, opcode, FIN, compression flag and unmasked Base64 payload. The UI offers text for complete uncompressed text frames, Hex/Base64, direction filters and Stop connection. Close is relayed both ways; cancellation terminates the transport and retains captured frames. Proxy shutdown also records a stopped session. The last queued frame is drained before normal completion.

This is frame capture, not a full WebSocket composer: active connections from the editor, message editing/replay, fragment reassembly, decompression, H2 extended CONNECT and H3 WebSocket are not implemented. Only rules matching this handshake and stage can block this initial path: nonempty enabled capture scripts or enabled capture-scoped interception are explicitly rejected when matched. Unrelated rules, empty scripts and replay-only interception do not block WebSocket capture. Response status conditions are evaluated after the upstream handshake response arrives. Native TLS is used for WSS, not browser fingerprint presets. Limits: 300 seconds per session, 8 MiB per frame, approximately 8 MiB total captured payload and 10000 frames. Stopping or exceeding limits closes the transport; it does not synthesize a graceful Close handshake.

H3 currently has a fixed-target loopback QUIC reverse proxy and an experimental dynamic-SNI TUN ingress. The latter has local tests but has not passed Windows end-to-end acceptance; investigation is paused as of 2026-10-08 (see [TUN notes](tun.md)). The fixed-target reverse proxy is separate from the system HTTP
proxy. Start it with a fixed origin (not a URL containing a path):

```powershell
& scripts/rust.ps1 run -p capture-service --bin quic-capture -- .local/capture https://example.com 8443
```

Use a client with HTTP/3 support and route its connection to 127.0.0.1:8443,
while retaining the origin hostname for TLS and HTTP authority. Trust the
CA printed by the process in that test client. For a curl build that supports
HTTP/3, for example:

```powershell
curl --http3-only --connect-to example.com:443:127.0.0.1:8443 --cacert .local/capture/certificates/capture-ca.pem https://example.com/
```

The target must actually support h3. Stock Windows curl on this development
machine does not support HTTP/3. Local Rust integration tests provide a QUIC
test peer. The reverse proxy writes capture records to the same database as
the development UI when given the same data directory. It does not install a
CA, modify system proxy settings, or transparently intercept browser UDP.

`clientProtocol` records the incoming protocol independently of the upstream
response version. Existing records remain readable. The capture table uses
the incoming protocol where available.

## Fidelity boundary

H1 capture retains raw request header bytes; native forwarding preserves the
relative order, casing, values and duplicates of end-to-end fields. Hop-by-hop
fields and Expect are removed; Host/Content-Length are corrected when needed,
and Connection: close is added. This is not byte-identical forwarding.

H2 capture uses the local h2 0.4.19 patch documented in
`vendor/h2/CAPTURE-PATCH.md`. Decoded HPACK field order is recorded before
HeaderMap iteration, including pseudo headers and interleaved duplicate fields.
`request.pseudoHeaders` stores pseudo fields in original order;
`request.headers` stores ordinary fields in original order. H2 forwarding uses
that order, preserves valid TE: trailers, and does not add missing Host or
Content-Length. Edited method/URL/body can require field-value changes.

`response.sentRequestHeaders` records the actual H1/H2 encoder input, including
pseudo fields for H2. The Header Order UI compares captured and transmitted
fields. H2 compressed HPACK bytes, dynamic table state, frame boundaries,
SETTINGS, priorities, TLS ClientHello and response forwarding order are not
claimed identical. Legacy H2 records without pseudoHeaders cannot recover their
original order. H3/QPACK order remains unsupported and explicitly marked.

## Upstream proxy forwarding

The proxy settings dialog accepts `http://host:port` (default 80) or
`socks5://host:port` (default 1080), with optional Basic / SOCKS username-password
authentication. Saved profiles persist locally; passwords optionally use the OS credential store.
The active global route applies to capture requests. Replay/composer requests
default to direct, or select a saved profile through upstreamProfileId. Routes
are snapshotted when execution starts and do not change during scripts or interception. Credentials never enter serialized Flow data; the status API does not
return passwords. A null password retains the existing password only for the same
proxy URL and username; disabling clears the configuration.

HTTP targets use absolute-form requests through an HTTP proxy. HTTPS targets use
CONNECT followed by the existing verified TLS + H1/H2 transport. SOCKS5 forwards
both HTTP and HTTPS streams, sending domain names to the proxy for resolution.
Authentication headers are sent only to the HTTP proxy, never through a tunnel
to the origin, and redacted in recorded transmitted headers. Failures never fall
back to direct access. wreq and httpcloak support HTTP/SOCKS5 upstreams. Native H3 supports SOCKS5 UDP ASSOCIATE; ordinary HTTP upstreams and CONNECT-UDP are unsupported.
Self-proxy loops are rejected before a request is written. End-to-end header
ordering remains intact; upstream proxies may independently rewrite traffic.

响应查看支持 gzip、deflate、br 和 zstd 解压；原始下载保留 Content-Encoding 对应的实体字节，解压下载保留解码后的二进制字节（不会写入格式化文本）。解压输出上限 8 MiB。
