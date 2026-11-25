package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"

	http "github.com/josexy/xhttp"
	"github.com/josexy/xhttp/httptrace"
	"golang.org/x/net/http/httpguts"
)

const (
	maxInputBytes = 48 << 20
	maxBodyBytes  = 32 << 20
)

func main() {
	input, err := readInput(os.Stdin)
	var response *engineResponse
	if err == nil {
		ctx, cancel := context.WithTimeout(context.Background(), 28*time.Second)
		defer cancel()
		response, err = send(ctx, input)
	}
	if err != nil {
		_ = json.NewEncoder(os.Stdout).Encode(engineReply{Error: err.Error()})
		return
	}
	_ = json.NewEncoder(os.Stdout).Encode(engineReply{Response: response})
}

func readInput(reader io.Reader) (engineInput, error) {
	var input engineInput
	limited := &io.LimitedReader{R: reader, N: maxInputBytes + 1}
	decoder := json.NewDecoder(limited)
	if err := decoder.Decode(&input); err != nil {
		return input, errors.New("invalid or oversized xhttp engine request")
	}
	if err := decoder.Decode(new(json.RawMessage)); err != io.EOF || limited.N == 0 {
		return input, errors.New("expected one xhttp engine request within the 48 MiB input limit")
	}
	return input, nil
}

func send(ctx context.Context, input engineInput) (*engineResponse, error) {
	tlsConfig, err := requestTLS(input)
	if err != nil {
		return nil, err
	}
	proxyURL, err := configuredProxy(input.Proxy)
	if err != nil {
		return nil, err
	}
	req, err := orderedRequest(ctx, input.Request, proxyURL)
	if err != nil {
		return nil, err
	}
	transport := &http.Transport{
		TLSClientConfig:        tlsConfig,
		ForceAttemptHTTP2:      true,
		DisableCompression:     true,
		TLSHandshakeTimeout:    10 * time.Second,
		MaxResponseHeaderBytes: 1 << 20,
	}
	if proxyURL != nil {
		transport.Proxy = http.ProxyURL(proxyURL)
	}
	defer transport.CloseIdleConnections()

	var mu sync.Mutex
	var writing, sent []engineHeader
	trace := &httptrace.ClientTrace{
		WroteHeaderField: func(name string, values []string) {
			mu.Lock()
			defer mu.Unlock()
			for _, value := range values {
				// Proxy authentication belongs to the tunnel, not the delivered request.
				if !strings.EqualFold(name, "Proxy-Authorization") {
					writing = append(writing, engineHeader{Name: name, Value: latin1Text(value)})
				}
			}
		},
		WroteHeaders: func() {
			mu.Lock()
			defer mu.Unlock()
			sent, writing = writing, nil
		},
	}
	resp, err := transport.RoundTrip(req.WithContext(httptrace.WithClientTrace(req.Context(), trace)))
	if err != nil {
		// RoundTrip errors omit the outer URL wrapper added by Client.Do.
		return nil, fmt.Errorf("xhttp request failed: %w", err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxBodyBytes+1))
	if err != nil {
		return nil, fmt.Errorf("read xhttp response: %w", err)
	}
	if len(body) > maxBodyBytes {
		return nil, errors.New("xhttp response exceeds the 32 MiB body limit")
	}
	headers, trailers, err := responseFields(resp)
	if err != nil {
		return nil, err
	}
	protocol := resp.Proto
	if resp.ProtoMajor == 2 {
		protocol = "HTTP/2"
	}
	result := &engineResponse{
		Status: resp.StatusCode, Version: protocol,
		Headers: headers, Trailers: trailers, BodyBase64: base64.StdEncoding.EncodeToString(body),
	}
	mu.Lock()
	result.SentRequestHeaders = append([]engineHeader(nil), sent...)
	mu.Unlock()
	if resp.TLS != nil {
		result.UpstreamTLS = observedTLS(resp.TLS)
		result.TLSVersion = result.UpstreamTLS.Version
	}
	return result, nil
}

func requestTLS(input engineInput) (*tls.Config, error) {
	options := input.Request.TLS
	if (input.Preset != "" && input.Preset != "native") || (options.Preset != "" && options.Preset != "native") {
		return nil, errors.New("xhttp supports only the native TLS profile")
	}
	if options.ClientHelloHex != nil || options.BrowserVersion != nil ||
		options.CipherList != nil || options.SigalgsList != nil || options.CurvesList != nil ||
		options.Grease != nil || options.PermuteExtensions != nil {
		return nil, errors.New("xhttp supports TLS version selection only")
	}
	config := &tls.Config{MinVersion: tls.VersionTLS12}
	switch options.Version {
	case "", "auto":
	case "1.2":
		config.MaxVersion = tls.VersionTLS12
	case "1.3":
		config.MinVersion = tls.VersionTLS13
	default:
		return nil, errors.New("invalid xhttp TLS version; choose auto, 1.2 or 1.3")
	}
	material := input.Security
	if material == nil {
		return config, nil
	}
	if len(material.TrustedCAPEM) > 0 {
		roots, err := x509.SystemCertPool()
		if err != nil {
			return nil, errors.New("cannot load system certificate roots")
		}
		for _, pem := range material.TrustedCAPEM {
			if !roots.AppendCertsFromPEM([]byte(pem)) {
				return nil, errors.New("invalid xhttp trusted CA")
			}
		}
		config.RootCAs = roots
	}
	if material.ClientCertificatePEM != "" || material.ClientPrivateKeyPEM != "" {
		identity, err := tls.X509KeyPair([]byte(material.ClientCertificatePEM), []byte(material.ClientPrivateKeyPEM))
		if err != nil {
			return nil, errors.New("invalid xhttp client certificate or private key")
		}
		config.Certificates = []tls.Certificate{identity}
	}
	return config, nil
}

func configuredProxy(proxy *engineProxy) (*url.URL, error) {
	if proxy == nil {
		return nil, nil
	}
	value, err := url.Parse(proxy.URL)
	if err != nil || value.Hostname() == "" || value.User != nil || value.Opaque != "" ||
		(value.Path != "" && value.Path != "/") || value.RawQuery != "" || value.Fragment != "" {
		return nil, errors.New("invalid xhttp proxy address")
	}
	switch value.Scheme {
	case "http", "socks5", "socks5h":
	default:
		return nil, errors.New("xhttp supports HTTP and SOCKS5 upstream proxies")
	}
	if proxy.Username != "" || proxy.Password != "" {
		value.User = url.UserPassword(proxy.Username, proxy.Password)
	}
	return value, nil
}

func orderedRequest(ctx context.Context, input engineRequest, proxyURL *url.URL) (*http.Request, error) {
	if !httpguts.ValidHeaderFieldName(input.Method) ||
		strings.EqualFold(input.Method, "CONNECT") || strings.EqualFold(input.Method, "TRACE") {
		return nil, errors.New("invalid or unsupported xhttp request method")
	}
	if base64.StdEncoding.DecodedLen(len(input.BodyBase64)) > maxBodyBytes+2 {
		return nil, errors.New("xhttp request exceeds the 32 MiB body limit")
	}
	body, err := base64.StdEncoding.DecodeString(input.BodyBase64)
	if err != nil || len(body) > maxBodyBytes {
		return nil, errors.New("invalid or oversized xhttp request body")
	}
	req, err := http.NewRequestWithContext(ctx, input.Method, input.URL, bytes.NewReader(body))
	if err != nil || req.URL.Hostname() == "" || req.URL.User != nil || req.URL.Opaque != "" || req.URL.Fragment != "" ||
		(req.URL.Scheme != "https" && req.URL.Scheme != "http") {
		return nil, errors.New("invalid xhttp request URL")
	}
	h1, h2, err := requestBlocks(req, input, len(body))
	if err != nil {
		return nil, err
	}
	// The exact HTTP/1 block must include transport-generated proxy auth for
	// absolute-form forward requests. HTTPS uses a separate CONNECT block.
	if req.URL.Scheme == "http" && proxyURL != nil && proxyURL.Scheme == "http" && proxyURL.User != nil {
		password, _ := proxyURL.User.Password()
		credentials := base64.StdEncoding.EncodeToString([]byte(proxyURL.User.Username() + ":" + password))
		h1.Fields = append(h1.Fields, http.HeaderField{Name: "Proxy-Authorization", Value: "Basic " + credentials})
	}
	req, err = http.WithRequestHeaderBlocks(req, h1, nil)
	if err == nil && req.URL.Scheme == "https" {
		req, err = http.WithRequestHeaderBlocks(req, h2, nil)
	}
	if err != nil {
		return nil, fmt.Errorf("prepare xhttp request header blocks: %w", err)
	}
	return req, nil
}

func requestBlocks(req *http.Request, input engineRequest, bodySize int) (http.HeaderBlock, http.HeaderBlock, error) {
	h1 := http.HeaderBlock{Kind: http.HeaderBlockInitial, ProtoMajor: 1}
	h2 := http.HeaderBlock{Kind: http.HeaderBlockInitial, ProtoMajor: 2}
	authority := req.URL.Host
	if len(input.Headers) > 258 || len(input.PseudoHeaders) > 4 {
		return h1, h2, errors.New("too many xhttp request fields")
	}
	pseudo, err := pseudoFields(req, input.PseudoHeaders)
	if err != nil {
		return h1, h2, err
	}
	h2.Fields = append(h2.Fields, pseudo...)
	// The Rust core already strips hop-by-hop fields. Repeat that check at the
	// process boundary so this helper can also be invoked on its own.
	drop := map[string]bool{
		"connection": true, "proxy-connection": true, "proxy-authorization": true,
		"proxy-authenticate": true, "keep-alive": true, "upgrade": true,
		"trailer": true, "transfer-encoding": true, "expect": true,
	}
	for _, field := range input.Headers {
		if !field.Disabled && strings.EqualFold(field.Name, "connection") {
			for _, token := range strings.Split(field.Value, ",") {
				drop[strings.ToLower(strings.TrimSpace(token))] = true
			}
		}
	}
	var hasHost, hasLength bool
	for _, field := range input.Headers {
		if field.Disabled {
			continue
		}
		value, err := latin1Bytes(field.Value)
		if !httpguts.ValidHeaderFieldName(field.Name) || err != nil || !httpguts.ValidHeaderFieldValue(value) {
			return h1, h2, errors.New("invalid xhttp request header")
		}
		name := strings.ToLower(field.Name)
		if drop[name] {
			continue
		}
		switch name {
		case "host":
			if hasHost {
				return h1, h2, errors.New("multiple Host fields")
			}
			hasHost = true
			h1.Fields = append(h1.Fields, http.HeaderField{Name: field.Name, Value: authority})
			continue
		case "content-length":
			if hasLength {
				return h1, h2, errors.New("multiple Content-Length fields")
			}
			hasLength = true
			value = strconv.Itoa(bodySize)
		case "te":
			if !strings.EqualFold(strings.TrimSpace(value), "trailers") {
				continue
			}
			value = "trailers"
			h2.Fields = append(h2.Fields, http.HeaderField{Name: name, Value: value})
			continue
		}
		h1.Fields = append(h1.Fields, http.HeaderField{Name: field.Name, Value: value})
		h2.Fields = append(h2.Fields, http.HeaderField{Name: name, Value: value, Sensitive: name == "authorization" || name == "cookie"})
	}
	if !hasHost {
		h1.Fields = append([]http.HeaderField{{Name: "Host", Value: authority}}, h1.Fields...)
	}
	if !hasLength {
		if bodySize > 0 || req.Method == "POST" || req.Method == "PUT" || req.Method == "PATCH" {
			value := strconv.Itoa(bodySize)
			h1.Fields = append(h1.Fields, http.HeaderField{Name: "Content-Length", Value: value})
			h2.Fields = append(h2.Fields, http.HeaderField{Name: "content-length", Value: value})
		}
	}
	h1.Fields = joinHTTP1Cookies(h1.Fields)
	return h1, h2, nil
}

func joinHTTP1Cookies(fields []http.HeaderField) []http.HeaderField {
	var cookies []string
	for _, field := range fields {
		if strings.EqualFold(field.Name, "cookie") {
			cookies = append(cookies, field.Value)
		}
	}
	if len(cookies) < 2 {
		return fields
	}
	out := fields[:0]
	for _, field := range fields {
		if strings.EqualFold(field.Name, "cookie") {
			if cookies == nil {
				continue
			}
			// RFC 9113 8.2.3 requires joining split cookies when writing H1.
			field.Value = strings.Join(cookies, "; ")
			cookies = nil
		}
		out = append(out, field)
	}
	return out
}

func pseudoFields(req *http.Request, input []engineHeader) ([]http.HeaderField, error) {
	defaultOrder := []string{":method", ":scheme", ":authority", ":path"}
	values := map[string]string{
		":method": req.Method, ":scheme": req.URL.Scheme, ":authority": req.URL.Host, ":path": req.URL.RequestURI(),
	}
	fields := make([]http.HeaderField, 0, 4)
	seen := make(map[string]bool, 4)
	for _, field := range input {
		if field.Disabled {
			continue
		}
		value, valid := values[field.Name]
		if !valid || seen[field.Name] {
			return nil, errors.New("unknown or repeated HTTP/2 pseudo-header")
		}
		seen[field.Name] = true
		fields = append(fields, http.HeaderField{Name: field.Name, Value: value})
	}
	for _, name := range defaultOrder {
		if !seen[name] {
			fields = append(fields, http.HeaderField{Name: name, Value: values[name]})
		}
	}
	return fields, nil
}

func responseFields(resp *http.Response) ([]engineHeader, []engineHeader, error) {
	headers, trailers := []engineHeader{}, []engineHeader{}
	seenInitial := false
	for _, block := range http.ResponseHeaderBlocks(resp) {
		switch block.Kind {
		case http.HeaderBlockInitial:
			seenInitial = true
			headers = appendResponseFields(headers, block.Fields)
		case http.HeaderBlockTrailer:
			trailers = appendResponseFields(trailers, block.Fields)
		}
	}
	if !seenInitial {
		return nil, nil, errors.New("xhttp did not return an observed response header block")
	}
	return headers, trailers, nil
}

func appendResponseFields(dst []engineHeader, fields []http.HeaderField) []engineHeader {
	for _, field := range fields {
		if !strings.HasPrefix(field.Name, ":") {
			dst = append(dst, engineHeader{Name: field.Name, Value: latin1Text(field.Value)})
		}
	}
	return dst
}

func latin1Bytes(text string) (string, error) {
	data := make([]byte, 0, len(text))
	for _, r := range text {
		if r > 255 {
			return "", errors.New("header contains non-Latin-1 data")
		}
		data = append(data, byte(r))
	}
	return string(data), nil
}

func latin1Text(data string) string {
	var text strings.Builder
	text.Grow(len(data))
	for i := range len(data) {
		text.WriteRune(rune(data[i]))
	}
	return text.String()
}

func observedTLS(state *tls.ConnectionState) *engineTLSDetails {
	details := &engineTLSDetails{
		Version: tls.VersionName(state.Version), CipherSuite: tls.CipherSuiteName(state.CipherSuite),
		ALPN: state.NegotiatedProtocol, ServerName: state.ServerName,
		OfferedCipherSuites: []string{}, OfferedALPN: []string{},
		SignatureSchemes: []string{}, SupportedGroups: []string{}, Certificates: []engineCertificate{},
	}
	for _, cert := range state.PeerCertificates {
		dns := append([]string{}, cert.DNSNames...)
		details.Certificates = append(details.Certificates, engineCertificate{
			Subject: cert.Subject.String(), Issuer: cert.Issuer.String(), Serial: cert.SerialNumber.Text(16),
			NotBefore: cert.NotBefore.UTC().Format(time.RFC3339), NotAfter: cert.NotAfter.UTC().Format(time.RFC3339),
			SHA256: fmt.Sprintf("%x", sha256.Sum256(cert.Raw)), DNSNames: dns, DERBase64: base64.StdEncoding.EncodeToString(cert.Raw),
		})
	}
	return details
}
