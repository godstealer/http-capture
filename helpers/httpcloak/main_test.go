package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestExactHeadersAndRedirect(t *testing.T) {
	l, e := net.Listen("tcp", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	defer l.Close()
	received := make(chan []string, 1)
	go func() {
		c, e := l.Accept()
		if e != nil {
			return
		}
		defer c.Close()
		r := bufio.NewReader(c)
		lines := []string{}
		for {
			line, e := r.ReadString('\n')
			if e != nil {
				return
			}
			if line == "\r\n" {
				break
			}
			lines = append(lines, strings.TrimSpace(line))
		}
		received <- lines
		fmt.Fprint(c, "HTTP/1.1 302 Found\r\nLocation: /must-not-follow\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest")
	}()
	in := Input{Preset: "chrome-150-windows", Request: Draft{Method: "GET", URL: "http://" + l.Addr().String() + "/", Headers: []Header{{"User-Agent", "Chrome/152.0.0.0"}, {"X-A", "1"}, {"X-B", "2"}, {"X-A", "3"}}}}
	out, e := execute(in)
	if e != nil {
		t.Fatal(e)
	}
	resp := out["response"].(map[string]any)
	if resp["status"] != 302 {
		t.Fatal(resp)
	}
	lines := <-received
	wanted := []string{"User-Agent: Chrome/152.0.0.0", "X-A: 1", "X-B: 2", "X-A: 3"}
	actual := []string{}
	for _, line := range lines {
		if strings.HasPrefix(line, "X-") || strings.HasPrefix(line, "User-Agent:") {
			actual = append(actual, line)
		}
	}
	if strings.Join(actual, "|") != strings.Join(wanted, "|") {
		t.Fatalf("header order: %v", lines)
	}
	for _, line := range lines {
		if strings.HasPrefix(strings.ToLower(line), "sec-ch-") {
			t.Fatal("preset injected header", line)
		}
	}
}
func TestDecodedResponseAndUntrustedTLS(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var b bytes.Buffer
		z := gzip.NewWriter(&b)
		z.Write([]byte("hello"))
		z.Close()
		w.Header().Set("Content-Encoding", "gzip")
		w.Write(b.Bytes())
	})
	s := httptest.NewServer(handler)
	defer s.Close()
	out, e := execute(Input{Preset: "chrome-152-windows", Request: Draft{Method: "GET", URL: s.URL, Headers: []Header{{"Accept-Encoding", "gzip"}}}})
	if e != nil {
		t.Fatal(e)
	}
	resp := out["response"].(map[string]any)
	if resp["bodyBase64"] != base64.StdEncoding.EncodeToString([]byte("hello")) {
		t.Fatal(resp)
	}
	for _, h := range resp["headers"].([]Header) {
		if strings.EqualFold(h.Name, "content-encoding") {
			t.Fatal("stale encoding header")
		}
	}
	tlsServer := httptest.NewTLSServer(handler)
	defer tlsServer.Close()
	_, e = execute(Input{Preset: "chrome-152-windows", Request: Draft{Method: "GET", URL: tlsServer.URL}})
	if e == nil {
		t.Fatal("untrusted certificate accepted")
	}
}
func TestUnknownProfile(t *testing.T) {
	if _, e := execute(Input{Preset: "chrome-999-windows"}); e == nil {
		t.Fatal("unsupported profile silently accepted")
	}
}

func TestCertificateWithoutDNSNamesUsesEmptyArray(t *testing.T) {
	data, e := json.Marshal(certificateDetails(&x509.Certificate{SerialNumber: big.NewInt(1)}))
	if e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(string(data), `"dnsNames":[]`) {
		t.Fatal(string(data))
	}
}
