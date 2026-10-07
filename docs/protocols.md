# HTTP/2 and HTTP/3 (experimental)

Auto is the default engine for newly composed requests: HTTPS advertises h2 and http/1.1 in a single TLS handshake; HTTP uses HTTP/1.1. ALPN chooses the upstream protocol before any HTTP request is sent. A failed request is never retried by changing protocols. The native engine explicitly selects HTTP/1.1. Select `h2` or `h3`
under Settings to send using that protocol. These engines validate the
server certificate, require HTTPS and default TLS settings, and fail rather
than silently falling back to HTTP/1.1. H3 sends QUIC over UDP directly to the
URL's port; it does not discover alternative ports through Alt-Svc.

The CONNECT proxy negotiates `h2` or `http/1.1` using ALPN. H2 connections
support concurrent streams and forward strictly with h2. H1 capture forwards with the native H1 engine; upstream h2 support does not upgrade a captured H1 request. Connections
currently have a 60-second lifetime. Bodies are limited to 8 MiB; trailers,
extended CONNECT, WebSockets and server push are not supported.

H3 capture uses an explicit loopback QUIC reverse proxy, not the system HTTP
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
back to direct access. H3, wreq and httpcloak do not support this route yet.
Self-proxy loops are rejected before a request is written. End-to-end header
ordering remains intact; upstream proxies may independently rewrite traffic.
