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
	Proxy   *Proxy `json:"proxy"`
	Request Draft  `json:"request"`
	Preset  string `json:"preset"`
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
	return map[string]any{"customClientHello": true, "protocolVersion": 1, "browserVersions": versions}
}
func execute(in Input) (map[string]any, error) {
	if fingerprint.GetStrict(in.Preset) == nil {
		return nil, fmt.Errorf("unsupported preset: %s", in.Preset)
	}
	customProtocol, customNote, err := applyClientHello(&in)
	if err != nil {
		return nil, err
	}
	body, err := base64.StdEncoding.DecodeString(in.Request.Body)
	if err != nil {
		return nil, err
	}
	if len(body) > 8*1024*1024 {
		return nil, fmt.Errorf("body exceeds 8 MiB")
	}
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
	var proxy *tr.ProxyConfig
	if in.Proxy != nil {
		if !strings.HasPrefix(in.Proxy.URL, "http://") && !strings.HasPrefix(in.Proxy.URL, "socks5://") {
			return nil, fmt.Errorf("unsupported upstream proxy scheme")
		}
		proxy = &tr.ProxyConfig{URL: in.Proxy.URL, Username: in.Proxy.Username, Password: in.Proxy.Password}
	}
	transport := tr.NewTransportWithConfig(in.Preset, proxy, &tr.TransportConfig{TLSOnly: true})
	defer transport.Close()
	// Explicit H2 for HTTPS, H1 for plaintext; no QUIC attempts or silent downgrade.
	transport.SetProtocol(customProtocol)
	if strings.HasPrefix(in.Request.URL, "http://") {
		transport.SetProtocol(tr.ProtocolHTTP1)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
	defer cancel()
	var tlsDetails map[string]any
	transport.SetTLSVerify(&tr.TLSVerify{VerifyConnection: func(s tls.ConnectionState) error {
		tlsDetails = map[string]any{"version": tls.VersionName(s.Version), "cipherSuite": tls.CipherSuiteName(s.CipherSuite), "alpn": s.NegotiatedProtocol, "serverName": s.ServerName, "offeredCipherSuites": []string{}, "offeredAlpn": []string{}, "signatureSchemes": []string{}, "supportedGroups": []string{}, "certificates": []any{}}
		certs := []any{}
		for _, c := range s.PeerCertificates {
			certs = append(certs, certificateDetails(c))
		}
		tlsDetails["certificates"] = certs
		return nil
	}})
	response, err := transport.DoStream(ctx, &tr.Request{Method: in.Request.Method, URL: in.Request.URL, Body: body, ExactHeaders: pairs, Timeout: 25 * time.Second})
	if err != nil {
		return nil, err
	}
	defer response.Close()
	data, err := io.ReadAll(io.LimitReader(response, 8*1024*1024+1))
	if err != nil {
		return nil, err
	}
	if len(data) > 8*1024*1024 {
		return nil, fmt.Errorf("response exceeds 8 MiB")
	}
	headers := []Header{}
	keys := make([]string, 0, len(response.Headers))
	for k := range response.Headers {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	decoded := false
	for _, k := range keys {
		if strings.EqualFold(k, "content-encoding") {
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
		headers = append(headers, Header{"content-length", strconv.Itoa(len(data))})
	}
	version := map[string]string{"h1": "HTTP/1.1", "h2": "HTTP/2", "h3": "HTTP/3"}[response.Protocol]
	var tlsVersion any
	if tlsDetails != nil {
		tlsVersion = tlsDetails["version"]
	}
	notes := []string{"httpcloak preset: " + in.Preset, "httpcloak ExactHeaders enabled; response headers are parsed fields, not certified wire order"}
	if customNote != "" {
		notes = append(notes, customNote)
	}
	if decoded {
		notes = append(notes, "httpcloak decoded response body; Content-Encoding removed and Content-Length updated")
	}
	return map[string]any{"response": map[string]any{"status": response.StatusCode, "version": version, "headers": headers, "bodyBase64": base64.StdEncoding.EncodeToString(data), "rawHeadBase64": nil, "sentRequestHeaders": nil, "tlsVersion": tlsVersion, "upstreamTls": tlsDetails}, "notes": notes}, nil
}
func main() {
	enc := json.NewEncoder(os.Stdout)
	if len(os.Args) > 1 && os.Args[1] == "--capabilities" {
		enc.Encode(capabilities())
		return
	}
	var input Input
	err := json.NewDecoder(io.LimitReader(os.Stdin, 16*1024*1024)).Decode(&input)
	var result map[string]any
	if err == nil {
		result, err = execute(input)
	}
	if err != nil {
		message := err.Error()
		if input.Proxy != nil {
			message = strings.ReplaceAll(message, input.Proxy.URL, "[upstream proxy]")
			if input.Proxy.Password != "" {
				message = strings.ReplaceAll(message, input.Proxy.Password, "[redacted]")
			}
		}
		enc.Encode(map[string]any{"error": message})
		return
	}
	enc.Encode(result)
}

func certificateDetails(c *x509.Certificate) map[string]any {
	return map[string]any{"subject": c.Subject.String(), "issuer": c.Issuer.String(), "serial": c.SerialNumber.Text(16), "notBefore": c.NotBefore.Format(time.RFC3339), "notAfter": c.NotAfter.Format(time.RFC3339), "sha256": fmt.Sprintf("%x", sha256.Sum256(c.Raw)), "dnsNames": append([]string{}, c.DNSNames...), "derBase64": base64.StdEncoding.EncodeToString(c.Raw), "parseError": nil}
}
