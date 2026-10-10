package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/hex"
	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"time"
)

func capturedHello(t *testing.T, alpn []string) []byte {
	t.Helper()
	a, b := net.Pipe()
	defer b.Close()
	go func() {
		defer a.Close()
		c := tls.Client(a, &tls.Config{ServerName: "example.com", NextProtos: alpn})
		_ = c.Handshake()
	}()
	b.SetDeadline(time.Now().Add(3 * time.Second))
	head := make([]byte, 5)
	if _, e := io.ReadFull(b, head); e != nil {
		t.Fatal(e)
	}
	raw := make([]byte, int(head[3])<<8|int(head[4]))
	if _, e := io.ReadFull(b, raw); e != nil {
		t.Fatal(e)
	}
	return append(head, raw...)
}
func TestImportedHelloHandshake(t *testing.T) {
	for _, h2 := range []bool{false, true} {
		alpn := []string{"http/1.1"}
		expectedProtocol := tr.ProtocolHTTP1
		expectedName := "h1"
		if h2 {
			alpn = []string{"h2", "http/1.1"}
			expectedProtocol = tr.ProtocolHTTP2
			expectedName = "h2"
		}
		raw := capturedHello(t, alpn)
		value := hex.EncodeToString(raw)
		in := Input{Preset: "chrome-152-windows"}
		in.Request.URL = "https://example.com"
		in.Request.TLS.ClientHelloHex = &value
		protocol, note, e := applyClientHello(&in)
		if e != nil {
			t.Fatal(e)
		}
		if protocol != expectedProtocol || !strings.Contains(note, "chrome-152") {
			t.Fatal(protocol, note)
		}
		server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("hello")) }))
		server.EnableHTTP2 = true
		server.StartTLS()
		defer server.Close()
		roots := x509.NewCertPool()
		roots.AddCert(server.Certificate())
		transport := tr.NewTransportWithConfig(in.Preset, nil, &tr.TransportConfig{TLSOnly: true})
		defer transport.Close()
		transport.SetProtocol(protocol)
		transport.SetTLSVerify(&tr.TLSVerify{RootCAs: roots})
		response, e := transport.DoStream(context.Background(), &tr.Request{Method: "GET", URL: server.URL, Timeout: 5 * time.Second})
		if e != nil {
			t.Fatal(e)
		}
		defer response.Close()
		body, e := io.ReadAll(response)
		if e != nil || string(body) != "hello" || response.Protocol != expectedName {
			t.Fatalf("%s %s %v", body, response.Protocol, e)
		}
		p := fingerprint.GetStrict(in.Preset)
		if len(p.RawClientHello) != len(raw) {
			t.Fatal("template not retained")
		}
	}
}
func TestRejectInvalidHello(t *testing.T) {
	for _, value := range []string{"", "zz", "1603010000", strings.Repeat("a", 400001)} {
		in := Input{Preset: "chrome-152-windows"}
		in.Request.URL = "https://example.com"
		in.Request.TLS.ClientHelloHex = &value
		if _, _, e := applyClientHello(&in); e == nil {
			t.Fatal("accepted invalid input")
		}
	}
}
