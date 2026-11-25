package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/pem"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	xhttp "github.com/josexy/xhttp"
	xhttptest "github.com/josexy/xhttp/httptest"
)

var orderedFixtureHeaders = []engineHeader{
	{Name: "X-Order", Value: "first"},
	{Name: "Cookie", Value: "a=1"},
	{Name: "X-Gap", Value: "middle"},
	{Name: "Cookie", Value: "b=2"},
	{Name: "X-Order", Value: "second"},
	{Name: "X-Disabled", Value: "never", Disabled: true},
}

var orderedFixtureWantH1 = []string{"x-order: first", "cookie: a=1; b=2", "x-gap: middle", "x-order: second"}
var orderedFixtureWantH2 = []string{"x-order: first", "cookie: a=1", "x-gap: middle", "cookie: b=2", "x-order: second"}

func fixtureInput(method, target string, body []byte) engineInput {
	return engineInput{
		Preset: "native",
		Request: engineRequest{
			Method: method, URL: target, TLS: engineTLS{Preset: "native"},
			BodyBase64: base64.StdEncoding.EncodeToString(body),
		},
	}
}

func fixtureSend(t *testing.T, input engineInput) (*engineResponse, error) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return send(ctx, input)
}

func fixtureOrderedHeaderLines(fields []engineHeader, wantedNames ...string) []string {
	var lines []string
	for _, field := range fields {
		name := strings.ToLower(field.Name)
		if slices.Contains(wantedNames, name) {
			lines = append(lines, name+": "+field.Value)
		}
	}
	return lines
}

func fixtureAssertLines(t *testing.T, got, want []string) {
	t.Helper()
	if !slices.Equal(got, want) {
		t.Fatalf("got fields %q, want %q", got, want)
	}
}

func TestHTTP1WireOrderBinaryTrailersAndRedirect(t *testing.T) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	payload := []byte{0, 'Q', 255}
	reply := []byte{0, 'R', 255}
	received := make(chan []string, 1)
	serverError := make(chan error, 1)
	go func() {
		serverError <- func() error {
			conn, err := listener.Accept()
			if err != nil {
				return err
			}
			defer conn.Close()
			if err := conn.SetDeadline(time.Now().Add(4 * time.Second)); err != nil {
				return err
			}
			reader := bufio.NewReader(conn)
			requestLine, err := reader.ReadString('\n')
			if err != nil || requestLine != "POST /first HTTP/1.1\r\n" {
				return fmt.Errorf("request line %q: %v", requestLine, err)
			}
			var lines []string
			for {
				line, err := reader.ReadString('\n')
				if err != nil {
					return err
				}
				if line == "\r\n" {
					break
				}
				name, value, ok := strings.Cut(strings.TrimSuffix(line, "\r\n"), ": ")
				if ok && slices.Contains([]string{"x-order", "x-gap", "x-disabled", "cookie"}, strings.ToLower(name)) {
					lines = append(lines, strings.ToLower(name)+": "+value)
				}
			}
			body := make([]byte, len(payload))
			if _, err := io.ReadFull(reader, body); err != nil || !bytes.Equal(body, payload) {
				return fmt.Errorf("request bytes %x: %v", body, err)
			}
			received <- lines
			_, err = fmt.Fprintf(conn, "HTTP/1.1 302 Found\r\nLocation: /must-not-follow\r\nX-Reply: first\r\nX-Gap: middle\r\nX-Reply: second\r\nTransfer-Encoding: chunked\r\nTrailer: X-Proof\r\nConnection: close\r\n\r\n3\r\n%s\r\n0\r\nX-Proof: complete\r\n\r\n", reply)
			return err
		}()
	}()

	input := fixtureInput("POST", "http://"+listener.Addr().String()+"/first", payload)
	input.Request.Headers = orderedFixtureHeaders
	response, err := fixtureSend(t, input)
	if err != nil {
		select {
		case fixtureErr := <-serverError:
			t.Fatalf("send failed: %v, fixture: %v", err, fixtureErr)
		default:
			t.Fatal(err)
		}
	}
	if err := <-serverError; err != nil {
		t.Fatal(err)
	}
	if response.Status != http.StatusFound || response.Version != "HTTP/1.1" || response.UpstreamTLS != nil || response.BodyBase64 != base64.StdEncoding.EncodeToString(reply) {
		t.Fatalf("unexpected response: %+v", response)
	}
	fixtureAssertLines(t, <-received, orderedFixtureWantH1)
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.SentRequestHeaders, "x-order", "x-gap", "x-disabled", "cookie"), orderedFixtureWantH1)
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.Headers, "x-reply", "x-gap"), []string{"x-reply: first", "x-gap: middle", "x-reply: second"})
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.Trailers, "x-proof"), []string{"x-proof: complete"})
}

func TestTLSHTTP2UsesTrustedCAAndPreservesWireOrder(t *testing.T) {
	type observation struct {
		blocks []xhttp.HeaderBlock
		body   []byte
		err    error
		alpn   string
	}
	observed := make(chan observation, 2)
	payload := []byte{0, 'H', '2', 255}
	server := xhttptest.NewUnstartedServer(xhttp.HandlerFunc(func(w xhttp.ResponseWriter, r *xhttp.Request) {
		body, err := io.ReadAll(r.Body)
		observed <- observation{xhttp.RequestHeaderBlocks(r), body, err, r.TLS.NegotiatedProtocol}
		w.Header().Add("X-Reply", "first")
		w.Header().Add("X-Reply", "second")
		w.Header().Set("Trailer", "X-Proof")
		_, _ = w.Write(body)
		w.Header().Set("X-Proof", "complete")
	}))
	server.EnableHTTP2 = true
	server.StartTLS()
	defer server.Close()

	input := fixtureInput("POST", server.URL+"/echo", payload)
	authority := strings.TrimPrefix(server.URL, "https://")
	input.Request.Headers = append(slices.Clone(orderedFixtureHeaders), engineHeader{Name: "Host", Value: authority})
	input.Request.PseudoHeaders = []engineHeader{
		{Name: ":path", Value: "/echo"},
		{Name: ":method", Value: "POST"},
		{Name: ":authority", Value: authority},
		{Name: ":scheme", Value: "https"},
	}
	if _, err := fixtureSend(t, input); err == nil {
		t.Fatal("accepted a server certificate outside the configured trust roots")
	}
	select {
	case <-observed:
		t.Fatal("untrusted connection reached the origin handler")
	default:
	}
	input.Security = &engineSecurity{TrustedCAPEM: []string{string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: server.Certificate().Raw}))}}
	response, err := fixtureSend(t, input)
	if err != nil {
		t.Fatal(err)
	}
	if response.Status != http.StatusOK || response.Version != "HTTP/2" || response.UpstreamTLS == nil || response.UpstreamTLS.ALPN != "h2" || response.TLSVersion == "" || response.BodyBase64 != base64.StdEncoding.EncodeToString(payload) {
		t.Fatalf("unexpected TLS/H2 response: %+v", response)
	}
	seen := <-observed
	if seen.err != nil || !bytes.Equal(seen.body, payload) || seen.alpn != "h2" {
		t.Fatalf("origin observation: bytes=%x, ALPN=%q, error=%v", seen.body, seen.alpn, seen.err)
	}
	var wireFields []engineHeader
	for _, block := range seen.blocks {
		if block.Kind == xhttp.HeaderBlockInitial {
			for _, field := range block.Fields {
				wireFields = append(wireFields, engineHeader{Name: field.Name, Value: field.Value})
			}
		}
	}
	fixtureAssertLines(t, fixtureOrderedHeaderLines(wireFields, "x-order", "x-gap", "x-disabled", "cookie", "host"), orderedFixtureWantH2)
	fixtureAssertLines(t, fixtureOrderedHeaderLines(wireFields, ":path", ":method", ":authority", ":scheme"), []string{":path: /echo", ":method: POST", ":authority: " + authority, ":scheme: https"})
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.SentRequestHeaders, "x-order", "x-gap", "x-disabled", "cookie"), orderedFixtureWantH2)
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.Headers, "x-reply"), []string{"x-reply: first", "x-reply: second"})
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.Trailers, "x-proof"), []string{"x-proof: complete"})
}

func TestHTTPConnectRefusalDoesNotFallBackToDirect(t *testing.T) {
	var originCalls, proxyCalls atomic.Int32
	origin := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		originCalls.Add(1)
		w.WriteHeader(http.StatusNoContent)
	}))
	defer origin.Close()
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		proxyCalls.Add(1)
		if r.Method != http.MethodConnect || r.Host != strings.TrimPrefix(origin.URL, "https://") {
			t.Errorf("unexpected proxy request: %s %s", r.Method, r.Host)
		}
		w.WriteHeader(http.StatusProxyAuthRequired)
	}))
	defer proxy.Close()
	input := fixtureInput("GET", origin.URL, nil)
	input.Security = &engineSecurity{TrustedCAPEM: []string{string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: origin.Certificate().Raw}))}}
	input.Proxy = &engineProxy{URL: proxy.URL}
	if _, err := fixtureSend(t, input); err == nil {
		t.Fatal("send succeeded despite rejected CONNECT")
	}
	if proxyCalls.Load() != 1 || originCalls.Load() != 0 {
		t.Fatalf("unexpected proxy/origin calls: %d / %d", proxyCalls.Load(), originCalls.Load())
	}
}

func TestHTTPForwardProxyAuthIsNotReportedAsOriginHeader(t *testing.T) {
	var originCalls, proxyCalls atomic.Int32
	origin := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		originCalls.Add(1)
		w.WriteHeader(http.StatusNoContent)
	}))
	defer origin.Close()
	const username, password = "fixture-user", "fixture-token"
	target := origin.URL + "/through-proxy"
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		proxyCalls.Add(1)
		if r.RequestURI != target || r.Method != http.MethodGet {
			t.Errorf("unexpected forward proxy target: %s %s", r.Method, r.RequestURI)
		}
		expectedAuth := "Basic " + base64.StdEncoding.EncodeToString([]byte(username+":"+password))
		if r.Header.Get("Proxy-Authorization") != expectedAuth {
			t.Error("proxy did not receive the configured authentication")
		}
		w.WriteHeader(http.StatusNonAuthoritativeInfo)
	}))
	defer proxy.Close()

	input := fixtureInput("GET", target, nil)
	input.Request.Headers = []engineHeader{{Name: "X-Proof", Value: "fixture"}}
	input.Proxy = &engineProxy{URL: proxy.URL, Username: username, Password: password}
	response, err := fixtureSend(t, input)
	if err != nil {
		t.Fatal(err)
	}
	if response.Status != http.StatusNonAuthoritativeInfo || proxyCalls.Load() != 1 || originCalls.Load() != 0 {
		t.Fatalf("unexpected status or proxy/origin calls: %d, %d / %d", response.Status, proxyCalls.Load(), originCalls.Load())
	}
	fixtureAssertLines(t, fixtureOrderedHeaderLines(response.SentRequestHeaders, "x-proof", "proxy-authorization"), []string{"x-proof: fixture"})
}

func TestHTTP2UsesConfiguredClientCertificate(t *testing.T) {
	publicKey, privateKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	client := &x509.Certificate{
		SerialNumber: big.NewInt(1), NotBefore: now.Add(-time.Hour), NotAfter: now.Add(time.Hour),
		BasicConstraintsValid: true, IsCA: true,
		KeyUsage:    x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign,
		ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth},
	}
	der, err := x509.CreateCertificate(rand.Reader, client, client, publicKey, privateKey)
	if err != nil {
		t.Fatal(err)
	}
	certificate, err := x509.ParseCertificate(der)
	if err != nil {
		t.Fatal(err)
	}
	keyDER, err := x509.MarshalPKCS8PrivateKey(privateKey)
	if err != nil {
		t.Fatal(err)
	}
	clientRoots := x509.NewCertPool()
	clientRoots.AddCert(certificate)

	var originCalls atomic.Int32
	server := xhttptest.NewUnstartedServer(xhttp.HandlerFunc(func(w xhttp.ResponseWriter, r *xhttp.Request) {
		originCalls.Add(1)
		if r.TLS == nil || len(r.TLS.PeerCertificates) != 1 || !bytes.Equal(r.TLS.PeerCertificates[0].Raw, der) {
			t.Error("origin did not receive the configured client certificate")
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	server.EnableHTTP2 = true
	server.TLS = &tls.Config{ClientAuth: tls.RequireAndVerifyClientCert, ClientCAs: clientRoots}
	server.StartTLS()
	defer server.Close()

	input := fixtureInput("GET", server.URL, nil)
	input.Security = &engineSecurity{TrustedCAPEM: []string{string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: server.Certificate().Raw}))}}
	input.Request.TLS.Version = "1.2"
	if _, err := fixtureSend(t, input); err == nil || originCalls.Load() != 0 {
		t.Fatal("mTLS origin accepted a request without a client certificate")
	}
	input.Security.ClientCertificatePEM = string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))
	input.Security.ClientPrivateKeyPEM = string(pem.EncodeToMemory(&pem.Block{Type: "PRIVATE KEY", Bytes: keyDER}))
	response, err := fixtureSend(t, input)
	if err != nil {
		t.Fatal(err)
	}
	if response.Status != http.StatusNoContent || response.Version != "HTTP/2" || response.TLSVersion != "TLS 1.2" || originCalls.Load() != 1 {
		t.Fatalf("unexpected mTLS response or origin count: %+v / %d", response, originCalls.Load())
	}
}
