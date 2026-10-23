# httpcloak transport adaptation

`httpcloak/` contains the production Go sources and embedded presets from
`github.com/sardanioss/httpcloak` v1.7.3. The upstream MIT license is kept at
`httpcloak/LICENSE`; examples, upstream tests and documentation are omitted.
The helper module uses a local Go `replace`, so builds and CI use this exact
source rather than a modified machine-local module cache.

Local source changes:

- `transport/tls_verify.go`: the existing verification hook also accepts a
  uTLS client certificate and installs it on the per-connection TLS config.
  It leaves ClientHello configuration unchanged. The helper parses the
  certificate and key before sending, and the application checks the helper
  capability before passing a client identity.
- `transport/stream.go`: proxied `DoStream` honors an explicit H1/H2/H3
  selection before the proxy's automatic protocol routing. The helper pins
  H1/H2 to avoid a SOCKS5 H3 race and to stream HTTPS responses without the
  buffered `Do` path. Proxy CONNECT and SOCKS5 H2 responses are tested for
  output before server EOF, alongside single-send ALPN downgrade tests.
- `transport/transport.go`: `Request.BodyLength` carries the validated decoded
  size for a streaming `BodyReader`. The helper reads Base64 from its existing
  JSON string in chunks, and `GetBody` reopens it for the pre-send H2-to-H1
  ALPN handoff without keeping a second full decoded byte slice.

When updating upstream, replace the copied sources from a pinned module
version, reapply/review these adaptations, update both Go modules and
`go.sum`, then run the helper's complete tests. The H1/H2 mutual TLS tests in
`helpers/httpcloak/main_test.go` must still pass before shipping.
