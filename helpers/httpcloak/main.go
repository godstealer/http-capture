package main

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
	utls "github.com/sardanioss/utls"
	"io"
	"os"
	"sort"
	"strconv"
	"strings"
	"time"
)

type Header struct {
	Name  string `json:"name"`
	Value string `json:"value"`
}
type Draft struct {
	TLS struct {
		ClientHelloHex *string `json:"clientHelloHex"`
	} `json:"tls"`
	Method  string   `json:"method"`
	URL     string   `json:"url"`
	Headers []Header `json:"headers"`
	Body    string   `json:"bodyBase64"`
}
type Proxy struct {
	URL      string `json:"url"`
	Username string `json:"username"`
	Password string `json:"password"`
}
type Input struct {
	Security *Security `json:"security,omitempty"`
	Proxy    *Proxy    `json:"proxy"`
	Request  Draft     `json:"request"`
	Preset   string    `json:"preset"`
	// Optional helper-level constraint. The application currently omits this
	// field, which means TCP ALPN negotiation; imported hellos remain strict.
	Protocol string `json:"protocol,omitempty"`
}
type Security struct {
	TrustedCAPEM         []string `json:"trustedCaPem"`
	ClientCertificatePEM *string  `json:"clientCertificatePem"`
	ClientPrivateKeyPEM  *string  `json:"clientPrivateKeyPem"`
}

func capabilities() map[string]any {
	versions := map[string][]int{}
	for _, family := range []string{"chrome", "firefox"} {
		for _, name := range fingerprint.Available() {
			parts := strings.Split(name, "-")
			if len(parts) != 3 || parts[0] != family || parts[2] != "windows" {
				continue
			}
			v, e := strconv.Atoi(parts[1])
			if e == nil {
				versions[family] = append(versions[family], v)
			}
		}
		sort.Sort(sort.Reverse(sort.IntSlice(versions[family])))
	}
	return map[string]any{"customClientHello": true, "certificateRoots": true, "clientCertificate": true, "protocolVersion": 1, "browserVersions": versions}
}
func execute(in Input) (map[string]any, error) {
	return executeWithRoots(in, nil)
}

// Custom roots extend platform trust; verification and hostname checks stay enabled.
func executeWithRoots(in Input, roots *x509.CertPool) (map[string]any, error) {
	result, err := executeWithRootsRaw(&in, roots)
	return materializeResult(result, err)
}

// The CLI owns its decoded JSON input. The encoded body is retained only by
// the streaming reader and its reopen callback until the send finishes.
func executeOwned(in *Input) (map[string]any, error) {
	result, err := executeWithRootsRaw(in, nil)
	return materializeResult(result, err)
}

type helperResult struct {
	metadata   map[string]any
	response   *negotiatedResponse
	preset     string
	customNote string
	tlsDetails map[string]any
	proxy      *Proxy
	cleanup    func()
}

func materializeResult(result *helperResult, err error) (map[string]any, error) {
	if err != nil {
		return nil, err
	}
	defer result.close()
	data, err := io.ReadAll(result.response)
	if err != nil {
		return nil, err
	}
	result.finishMetadata(int64(len(data)))
	result.metadata["response"].(map[string]any)["bodyBase64"] = base64.StdEncoding.EncodeToString(data)
	return result.metadata, nil
}

func (result *helperResult) close() {
	if result.cleanup != nil {
		result.cleanup()
		result.cleanup = nil
	} else if result.response != nil {
		result.response.Close()
	}
}

func executeWithRootsRaw(in *Input, roots *x509.CertPool) (*helperResult, error) {
	var clientCertificate *utls.Certificate
	if in.Security != nil {
		certPEM, keyPEM := in.Security.ClientCertificatePEM, in.Security.ClientPrivateKeyPEM
		if (certPEM == nil) != (keyPEM == nil) {
			return nil, fmt.Errorf("client certificate and private key must be provided together")
		}
		if certPEM != nil {
			if len(*certPEM) > 1024*1024 || len(*keyPEM) > 1024*1024 {
				return nil, fmt.Errorf("client certificate or private key is too large")
			}
			parsed, err := utls.X509KeyPair([]byte(*certPEM), []byte(*keyPEM))
			if err != nil {
				return nil, fmt.Errorf("invalid client certificate or private key: %w", err)
			}
			clientCertificate = &parsed
		}
		if len(in.Security.TrustedCAPEM) > 64 {
			return nil, fmt.Errorf("too many custom trust certificates")
		}
		if len(in.Security.TrustedCAPEM) > 0 {
			if roots == nil {
				var err error
				roots, err = x509.SystemCertPool()
				if err != nil {
					return nil, fmt.Errorf("cannot read system trust pool")
				}
			} else {
				roots = roots.Clone()
			}
			for _, value := range in.Security.TrustedCAPEM {
				if len(value) > 1024*1024 || !roots.AppendCertsFromPEM([]byte(value)) {
					return nil, fmt.Errorf("invalid custom trust certificate")
				}
			}
		}
	}
	if fingerprint.GetStrict(in.Preset) == nil {
		return nil, fmt.Errorf("unsupported preset: %s", in.Preset)
	}
	originalPreset := in.Preset
	customProtocol, customNote, err := applyClientHello(in)
	if err != nil {
		return nil, err
	}
	if in.Preset != originalPreset {
		defer fingerprint.Unregister(in.Preset)
	}
	transportPreset, err := exactCookiePreset(in.Preset)
	if err != nil {
		return nil, err
	}
	defer fingerprint.Unregister(transportPreset)
	encodedBody := in.Request.Body
	newBody := func() (io.Reader, error) {
		return base64.NewDecoder(base64.StdEncoding, strings.NewReader(encodedBody)), nil
	}
	bodyLength, err := io.Copy(io.Discard, base64.NewDecoder(base64.StdEncoding, strings.NewReader(encodedBody)))
	if err != nil {
		return nil, err
	}
	in.Request.Body = ""
	pairs := make([]fingerprint.HeaderPair, 0, len(in.Request.Headers))
	for _, h := range in.Request.Headers {
		value := []byte{}
		for _, c := range h.Value {
			if c > 255 || c == 10 || c == 13 || c == 0 {
				return nil, fmt.Errorf("invalid header value")
			}
			value = append(value, byte(c))
		}
		pairs = append(pairs, fingerprint.HeaderPair{Key: h.Name, Value: string(value)})
	}
	protocol, negotiateHTTP1, err := selectRequestProtocol(*in, customProtocol)
	if err != nil {
		return nil, err
	}
	if protocol == tr.ProtocolHTTP1 {
		pairs, err = prepareH1Headers(pairs, bodyLength)
		if err != nil {
			return nil, err
		}
	}
	// The forked H2 encoder synthesizes Go-http-client/2.0 unless its header
	// map contains User-Agent. An empty value suppresses that default on the
	// wire; retain the original pairs for a possible ALPN handoff to H1.
	wirePairs := pairs
	if protocol == tr.ProtocolHTTP2 {
		hasUserAgent := false
		for _, pair := range pairs {
			if strings.EqualFold(pair.Key, "user-agent") {
				hasUserAgent = true
				break
			}
		}
		if !hasUserAgent {
			wirePairs = append(append([]fingerprint.HeaderPair(nil), pairs...), fingerprint.HeaderPair{Key: "user-agent", Value: ""})
		}
	}
	var proxy *tr.ProxyConfig
	if in.Proxy != nil {
		if !strings.HasPrefix(in.Proxy.URL, "http://") && !strings.HasPrefix(in.Proxy.URL, "socks5://") {
			return nil, fmt.Errorf("unsupported upstream proxy scheme")
		}
		proxy = &tr.ProxyConfig{URL: in.Proxy.URL, Username: in.Proxy.Username, Password: in.Proxy.Password}
	}
	transport := tr.NewTransportWithConfig(transportPreset, proxy, &tr.TransportConfig{TLSOnly: true})
	owned := true
	// Start with browser TLS over TCP. Only a typed, pre-request ALPN mismatch
	// may select H1 in default mode; never use the library's H3/response fallback.
	transport.SetProtocol(protocol)
	ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
	defer func() {
		if owned {
			transport.Close()
			cancel()
		}
	}()
	var tlsDetails map[string]any
	transport.SetTLSVerify(&tr.TLSVerify{RootCAs: roots, ClientCertificate: clientCertificate, VerifyConnection: func(s tls.ConnectionState) error {
		tlsDetails = map[string]any{"version": tls.VersionName(s.Version), "cipherSuite": tls.CipherSuiteName(s.CipherSuite), "alpn": s.NegotiatedProtocol, "serverName": s.ServerName, "offeredCipherSuites": []string{}, "offeredAlpn": []string{}, "signatureSchemes": []string{}, "supportedGroups": []string{}, "certificates": []any{}}
		certs := []any{}
		for _, c := range s.PeerCertificates {
			certs = append(certs, certificateDetails(c))
		}
		tlsDetails["certificates"] = certs
		return nil
	}})
	request := &tr.Request{Method: in.Request.Method, URL: in.Request.URL, ExactHeaders: wirePairs, Timeout: 25 * time.Second, BodyLength: bodyLength}
	if bodyLength > 0 {
		request.BodyReader, _ = newBody()
		request.GetBody = newBody
	}
	response, err := sendNegotiated(ctx, transport, request, protocol, negotiateHTTP1, pairs)
	if err != nil {
		return nil, err
	}
	owned = false
	return &helperResult{
		response: response, preset: in.Preset, customNote: customNote,
		tlsDetails: tlsDetails, proxy: in.Proxy,
		cleanup: func() { response.Close(); transport.Close(); cancel() },
	}, nil
}

func (result *helperResult) finishMetadata(bodyLength int64) {
	if result.metadata != nil {
		return
	}
	response := result.response
	tlsDetails := result.tlsDetails
	headers := []Header{}
	keys := make([]string, 0, len(response.Headers))
	for k := range response.Headers {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	decoded := false
	for _, k := range keys {
		if response.decodedBody && strings.EqualFold(k, "content-encoding") {
			for _, v := range response.Headers[k] {
				switch strings.ToLower(v) {
				case "gzip", "br", "deflate", "zstd":
					decoded = true
				}
			}
		}
	}
	for _, k := range keys {
		if decoded && (strings.EqualFold(k, "content-encoding") || strings.EqualFold(k, "content-length")) {
			continue
		}
		for _, v := range response.Headers[k] {
			runes := make([]rune, len(v))
			for i, b := range []byte(v) {
				runes[i] = rune(b)
			}
			headers = append(headers, Header{k, string(runes)})
		}
	}
	if decoded {
		headers = append(headers, Header{"content-length", strconv.FormatInt(bodyLength, 10)})
	}
	version := map[string]string{"h1": "HTTP/1.1", "h2": "HTTP/2", "h3": "HTTP/3"}[response.Protocol]
	var tlsVersion any
	if tlsDetails != nil {
		tlsVersion = tlsDetails["version"]
	}
	notes := []string{"httpcloak preset: " + result.preset, "httpcloak ExactHeaders enabled; response headers are parsed fields, not certified wire order"}
	if response.Protocol == "h2" {
		notes = append(notes, "httpcloak HTTP/2 Cookie fields follow the editor; automatic Cookie splitting is disabled")
	} else if response.Protocol == "h1" {
		notes = append(notes, "httpcloak HTTP/1.1 joins repeated Cookie fields with '; ' at the first Cookie position; original fields remain in the editor")
	}
	if result.customNote != "" {
		notes = append(notes, result.customNote)
	}
	if decoded {
		notes = append(notes, "httpcloak decoded response body; Content-Encoding removed and Content-Length updated")
	}
	result.metadata = map[string]any{"response": map[string]any{"status": response.StatusCode, "version": version, "headers": headers, "rawHeadBase64": nil, "sentRequestHeaders": nil, "tlsVersion": tlsVersion, "upstreamTls": tlsDetails}, "notes": notes}
}

// Stream the network body before writing the final metadata. A read failure
// leaves an explicit top-level error, so a partial body cannot be accepted.
func writeResponse(w io.Writer, result *helperResult) error {
	defer result.close()
	if _, err := io.WriteString(w, `{"response":{"bodyBase64":"`); err != nil {
		return err
	}
	encoded := base64.NewEncoder(base64.StdEncoding, w)
	bodyLength, readErr := io.Copy(encoded, result.response)
	if err := encoded.Close(); err != nil {
		return err
	}
	result.finishMetadata(bodyLength)
	fields, err := json.Marshal(result.metadata["response"])
	if err != nil {
		return err
	}
	if len(fields) < 2 || fields[0] != '{' || fields[len(fields)-1] != '}' {
		return fmt.Errorf("invalid response metadata")
	}
	if _, err = io.WriteString(w, `"`); err != nil {
		return err
	}
	if len(fields) > 2 {
		if _, err = io.WriteString(w, ","); err != nil {
			return err
		}
		if _, err = w.Write(fields[1 : len(fields)-1]); err != nil {
			return err
		}
	}
	notes, err := json.Marshal(result.metadata["notes"])
	if err != nil {
		return err
	}
	if _, err = io.WriteString(w, `},"notes":`); err != nil {
		return err
	}
	if _, err = w.Write(notes); err != nil {
		return err
	}
	if readErr != nil {
		message, err := json.Marshal(redactProxyError(readErr, result.proxy))
		if err != nil {
			return err
		}
		if _, err = io.WriteString(w, `,"error":`); err != nil {
			return err
		}
		if _, err = w.Write(message); err != nil {
			return err
		}
	}
	_, err = io.WriteString(w, "}\n")
	return err
}

func redactProxyError(err error, proxy *Proxy) string {
	message := err.Error()
	if proxy != nil {
		message = strings.ReplaceAll(message, proxy.URL, "[upstream proxy]")
		if proxy.Password != "" {
			message = strings.ReplaceAll(message, proxy.Password, "[redacted]")
		}
	}
	return message
}
func main() {
	enc := json.NewEncoder(os.Stdout)
	if len(os.Args) > 1 && os.Args[1] == "--capabilities" {
		enc.Encode(capabilities())
		return
	}
	var input Input
	err := json.NewDecoder(os.Stdin).Decode(&input)
	var result *helperResult
	if err == nil {
		result, err = executeWithRootsRaw(&input, nil)
	}
	if err != nil {
		enc.Encode(map[string]any{"error": redactProxyError(err, input.Proxy)})
		return
	}
	if err := writeResponse(os.Stdout, result); err != nil {
		os.Exit(1)
	}
}

func certificateDetails(c *x509.Certificate) map[string]any {
	return map[string]any{"subject": c.Subject.String(), "issuer": c.Issuer.String(), "serial": c.SerialNumber.Text(16), "notBefore": c.NotBefore.Format(time.RFC3339), "notAfter": c.NotAfter.Format(time.RFC3339), "sha256": fmt.Sprintf("%x", sha256.Sum256(c.Raw)), "dnsNames": append([]string{}, c.DNSNames...), "derBase64": base64.StdEncoding.EncodeToString(c.Raw), "parseError": nil}
}
