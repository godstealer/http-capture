package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
)

type alpnH1Request struct {
	head []string
	body []byte
	alpn string
}
type alpnH1Fixture struct {
	url      string
	roots    *x509.CertPool
	hellos   atomic.Int32
	requests atomic.Int32
	seen     chan alpnH1Request
	closed   chan struct{}
}

func newALPNH1Fixture(t *testing.T, alpn []string, reply []byte) *alpnH1Fixture {
	t.Helper()
	certificateSource := httptest.NewTLSServer(nil)
	certificate := certificateSource.TLS.Certificates[0]
	roots := x509.NewCertPool()
	roots.AddCert(certificateSource.Certificate())
	certificateSource.Close()
	fixture := &alpnH1Fixture{roots: roots, seen: make(chan alpnH1Request, 8), closed: make(chan struct{}, 8)}
	listener, err := tls.Listen("tcp", "127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{certificate}, NextProtos: alpn,
		GetConfigForClient: func(*tls.ClientHelloInfo) (*tls.Config, error) { fixture.hellos.Add(1); return nil, nil }})
	if err != nil {
		t.Fatal(err)
	}
	fixture.url = "https://" + listener.Addr().String() + "/post"
	var mu sync.Mutex
	connections := []net.Conn{}
	t.Cleanup(func() {
		listener.Close()
		mu.Lock()
		defer mu.Unlock()
		for _, connection := range connections {
			connection.Close()
		}
	})
	go func() {
		for {
			connection, err := listener.Accept()
			if err != nil {
				return
			}
			mu.Lock()
			connections = append(connections, connection)
			mu.Unlock()
			go func() {
				defer connection.Close()
				defer func() { fixture.closed <- struct{}{} }()
				connection.SetDeadline(time.Now().Add(5 * time.Second))
				tlsConnection := connection.(*tls.Conn)
				if tlsConnection.Handshake() != nil {
					return
				}
				reader := bufio.NewReader(connection)
				head := []string{}
				length := 0
				for {
					line, err := reader.ReadString('\n')
					if err != nil {
						return
					}
					if line == "\r\n" {
						break
					}
					line = strings.TrimSuffix(strings.TrimSuffix(line, "\n"), "\r")
					head = append(head, line)
					if name, value, ok := strings.Cut(line, ":"); ok && strings.EqualFold(name, "content-length") {
						length, err = strconv.Atoi(strings.TrimSpace(value))
						if err != nil || length < 0 || length > 65536 {
							return
						}
					}
				}
				body := make([]byte, length)
				if _, err := io.ReadFull(reader, body); err != nil {
					return
				}
				fixture.requests.Add(1)
				fixture.seen <- alpnH1Request{head, body, tlsConnection.ConnectionState().NegotiatedProtocol}
				if len(reply) == 0 {
					return
				}
				if _, err := connection.Write(reply); err != nil {
					return
				}
				// The client must release the handed-off socket on success/error.
				io.Copy(io.Discard, reader)
			}()
		}
	}()
	return fixture
}

func awaitClosed(t *testing.T, fixture *alpnH1Fixture) {
	t.Helper()
	select {
	case <-fixture.closed:
	case <-time.After(2 * time.Second):
		t.Fatal("TLS connection was not closed")
	}
}

func TestAutoALPNH1SendsPostOnceAndKeepsExactHeaders(t *testing.T) {
	var compressed bytes.Buffer
	compressor := gzip.NewWriter(&compressed)
	compressor.Write([]byte("encoded response"))
	compressor.Close()
	reply := append([]byte(fmt.Sprintf("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: %d\r\n\r\n", compressed.Len())), compressed.Bytes()...)
	for _, base := range []string{"chrome-152-windows", "chrome-155-windows", "firefox-148-windows"} {
		for _, alpn := range [][]string{{"http/1.1"}, nil} {
			t.Run(fmt.Sprintf("%s/%v", base, alpn), func(t *testing.T) {
				fixture := newALPNH1Fixture(t, alpn, reply)
				input := Input{Preset: base, Request: Draft{Method: "POST", URL: fixture.url, Body: base64.StdEncoding.EncodeToString([]byte("send exactly once")),
					Headers: []Header{{"X-Start", "one"}, {"Cookie", "same=1; quoted=\"x;y\""}, {"X-Middle", "middle"}, {"cookie", "same=2; empty="}, {"X-Start", "two"}}}}
				before := append([]Header(nil), input.Request.Headers...)
				result, err := executeWithRoots(input, fixture.roots)
				if err != nil {
					t.Fatal(err)
				}
				actual := <-fixture.seen
				want := []string{"X-Start: one", "Cookie: same=1; quoted=\"x;y\"; same=2; empty=", "X-Middle: middle", "X-Start: two"}
				got := []string{}
				for _, line := range actual.head {
					if strings.HasPrefix(line, "X-") || strings.HasPrefix(line, "Cookie:") || strings.HasPrefix(line, "cookie:") {
						got = append(got, line)
					}
					if strings.Contains(line, "Exact-Headers:") || strings.HasPrefix(line, "Connection:") {
						t.Fatal("synthesized/exact marker header leaked", line)
					}
				}
				if !reflect.DeepEqual(got, want) || string(actual.body) != "send exactly once" || !reflect.DeepEqual(input.Request.Headers, before) {
					t.Fatalf("wire changed: %v %q", got, actual.body)
				}
				response := result["response"].(map[string]any)
				if response["version"] != "HTTP/1.1" || response["bodyBase64"] != base64.StdEncoding.EncodeToString(compressed.Bytes()) {
					t.Fatal("raw H1 response was corrupted", response)
				}
				if response["upstreamTls"].(map[string]any)["alpn"] != actual.alpn || len(response["upstreamTls"].(map[string]any)["certificates"].([]any)) == 0 {
					t.Fatal("actual verified TLS state missing")
				}
				foundEncoding, foundLength := false, false
				for _, header := range response["headers"].([]Header) {
					if strings.EqualFold(header.Name, "content-encoding") {
						foundEncoding = header.Value == "gzip"
					}
					if strings.EqualFold(header.Name, "content-length") {
						foundLength = header.Value == strconv.Itoa(compressed.Len())
					}
				}
				if !foundEncoding || !foundLength {
					t.Fatal("raw compressed body's headers were removed")
				}
				awaitClosed(t, fixture)
				if fixture.hellos.Load() != 1 || fixture.requests.Load() != 1 {
					t.Fatal("handshake/POST repeated", fixture.hellos.Load(), fixture.requests.Load())
				}
			})
		}
	}
}

func TestAutoALPNKeepsHTTP2CookieWireFields(t *testing.T) {
	for _, base := range []string{"chrome-152-windows", "chrome-155-windows", "firefox-148-windows"} {
		t.Run(base, func(t *testing.T) {
			url, roots, received := h2CookieWireServer(t)
			result, err := executeWithRoots(Input{Preset: base, Request: Draft{Method: "GET", URL: url,
				Headers: []Header{{"user-agent", "alpn-test"}, {"cookie", "same=1; same=2"}, {"x-between", "middle"}, {"cookie", "empty="}}}}, roots)
			if err != nil {
				t.Fatal(err)
			}
			fields := <-received
			if fields.err != nil {
				t.Fatal(fields.err)
			}
			actual := []string{}
			for _, field := range fields.fields {
				if !strings.HasPrefix(field.Name, ":") {
					actual = append(actual, field.Name+":"+field.Value)
				}
			}
			if !reflect.DeepEqual(actual, []string{"user-agent:alpn-test", "cookie:same=1; same=2", "x-between:middle", "cookie:empty="}) {
				t.Fatal("H2 Cookie boundaries/order changed", actual)
			}
			response := result["response"].(map[string]any)
			if response["version"] != "HTTP/2" || response["upstreamTls"].(map[string]any)["alpn"] != "h2" {
				t.Fatal("H2 negotiation changed")
			}
		})
	}
}

func TestALPNStrictChoicesAndUntrustedCertificateNeverFallBack(t *testing.T) {
	for _, mode := range []string{"forced-h2", "import-h2", "import-h2-h1", "untrusted"} {
		t.Run(mode, func(t *testing.T) {
			fixture := newALPNH1Fixture(t, []string{"http/1.1"}, []byte("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"))
			input := Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: fixture.url, Body: "b25jZQ=="}}
			roots := fixture.roots
			switch mode {
			case "forced-h2":
				input.Protocol = "h2"
			case "import-h2", "import-h2-h1":
				alpn := []string{"h2"}
				if mode == "import-h2-h1" {
					alpn = append(alpn, "http/1.1")
				}
				value := hex.EncodeToString(capturedHello(t, alpn))
				input.Request.TLS.ClientHelloHex = &value
			case "untrusted":
				roots = nil
			}
			if _, err := executeWithRoots(input, roots); err == nil {
				t.Fatal("strict/untrusted negotiation succeeded")
			}
			awaitClosed(t, fixture)
			if fixture.requests.Load() != 0 || fixture.hellos.Load() != 1 {
				t.Fatal("failed handshake retried/sent a request", fixture.requests.Load(), fixture.hellos.Load())
			}
		})
	}
}

func TestNegotiatedPostResponseErrorsDoNotReplay(t *testing.T) {
	t.Run("h1", func(t *testing.T) {
		fixture := newALPNH1Fixture(t, []string{"http/1.1"}, nil)
		if _, err := executeWithRoots(Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: fixture.url, Body: "b25jZQ=="}}, fixture.roots); err == nil {
			t.Fatal("closed response accepted")
		}
		awaitClosed(t, fixture)
		if fixture.requests.Load() != 1 || fixture.hellos.Load() != 1 {
			t.Fatal("H1 POST replayed after a response error")
		}
	})
	t.Run("h2", func(t *testing.T) {
		var requests, hellos atomic.Int32
		server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			body, err := io.ReadAll(r.Body)
			if err != nil || string(body) != "once" {
				t.Error("POST body changed", err)
			}
			requests.Add(1)
			panic(http.ErrAbortHandler)
		}))
		server.EnableHTTP2 = true
		server.TLS = &tls.Config{GetConfigForClient: func(*tls.ClientHelloInfo) (*tls.Config, error) { hellos.Add(1); return nil, nil }}
		server.StartTLS()
		defer server.Close()
		roots := x509.NewCertPool()
		roots.AddCert(server.Certificate())
		if _, err := executeWithRoots(Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: server.URL, Body: "b25jZQ=="}}, roots); err == nil {
			t.Fatal("reset stream accepted")
		}
		if requests.Load() != 1 || hellos.Load() != 1 {
			t.Fatal("H2 POST replayed after a stream error", requests.Load(), hellos.Load())
		}
	})
}

func TestAutoALPNReusesOneProxyTunnel(t *testing.T) {
	fixture := newALPNH1Fixture(t, []string{"http/1.1"}, []byte("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"))
	var connects atomic.Int32
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "CONNECT" || r.Host != "127.0.0.1:1" {
			t.Error("unexpected proxy route", r.Method, r.Host)
			w.WriteHeader(400)
			return
		}
		connects.Add(1)
		upstream, err := net.Dial("tcp", strings.TrimPrefix(strings.TrimSuffix(fixture.url, "/post"), "https://"))
		if err != nil {
			w.WriteHeader(502)
			return
		}
		client, buffered, err := w.(http.Hijacker).Hijack()
		if err != nil {
			upstream.Close()
			return
		}
		defer client.Close()
		defer upstream.Close()
		fmt.Fprint(client, "HTTP/1.1 200 Connection Established\r\n\r\n")
		go io.Copy(upstream, buffered)
		io.Copy(client, upstream)
	}))
	defer proxy.Close()
	result, err := executeWithRoots(Input{Preset: "chrome-152-windows", Proxy: &Proxy{URL: proxy.URL},
		Request: Draft{Method: "POST", URL: "https://127.0.0.1:1/post", Body: "b25jZQ==", Headers: []Header{{"Cookie", "a=1"}, {"X-Middle", "middle"}, {"cookie", "b=2"}}}}, fixture.roots)
	if err != nil {
		t.Fatal(err)
	}
	awaitClosed(t, fixture)
	if connects.Load() != 1 || fixture.hellos.Load() != 1 || fixture.requests.Load() != 1 {
		t.Fatal("ALPN handoff redialed or bypassed proxy", connects.Load(), fixture.hellos.Load(), fixture.requests.Load())
	}
	if result["response"].(map[string]any)["version"] != "HTTP/1.1" {
		t.Fatal("proxy protocol not reported")
	}
}

func TestALPNHandoffCancellationClosesSocket(t *testing.T) {
	fixture := newALPNH1Fixture(t, []string{"http/1.1"}, []byte("HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n"))
	name, err := exactCookiePreset("chrome-152-windows")
	if err != nil {
		t.Fatal(err)
	}
	defer fingerprint.Unregister(name)
	transport := tr.NewTransportWithConfig(name, nil, &tr.TransportConfig{TLSOnly: true})
	defer transport.Close()
	transport.SetProtocol(tr.ProtocolHTTP2)
	transport.SetTLSVerify(&tr.TLSVerify{RootCAs: fixture.roots})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	response, err := sendNegotiated(ctx, transport, &tr.Request{Method: "POST", URL: fixture.url, Body: []byte("once"), Timeout: 3 * time.Second}, tr.ProtocolHTTP2, true, false)
	if err != nil {
		t.Fatal(err)
	}
	cancel()
	if _, err := io.ReadAll(response); err == nil {
		t.Fatal("cancelled incomplete response succeeded")
	}
	response.Close()
	awaitClosed(t, fixture)
	if fixture.hellos.Load() != 1 || fixture.requests.Load() != 1 {
		t.Fatal("cancelled POST replayed")
	}
}

// SOCKS5 normally makes the upstream DoStream take its Auto/H3 branch, even
// after SetProtocol(H2). Refuse UDP ASSOCIATE and observe every command, so a
// protocol race or an extra tunnel cannot hide behind a successful response.
func socksALPNProxy(t *testing.T, destination string) (string, *atomic.Int32, *atomic.Int32) {
	t.Helper()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var connects, unexpected atomic.Int32
	var mu sync.Mutex
	connections := []net.Conn{}
	t.Cleanup(func() {
		listener.Close()
		mu.Lock()
		defer mu.Unlock()
		for _, connection := range connections {
			connection.Close()
		}
	})
	go func() {
		for {
			client, err := listener.Accept()
			if err != nil {
				return
			}
			mu.Lock()
			connections = append(connections, client)
			mu.Unlock()
			go func() {
				defer client.Close()
				client.SetDeadline(time.Now().Add(5 * time.Second))
				var header [4]byte
				if _, err := io.ReadFull(client, header[:2]); err != nil {
					return
				}
				if header[0] != 5 {
					unexpected.Add(1)
					return
				}
				methods := make([]byte, int(header[1]))
				if _, err := io.ReadFull(client, methods); err != nil {
					return
				}
				client.Write([]byte{5, 0})
				if _, err := io.ReadFull(client, header[:]); err != nil {
					return
				}
				if header[0] != 5 || header[1] != 1 || header[2] != 0 {
					unexpected.Add(1)
					return
				}
				var host string
				switch header[3] {
				case 1:
					var ip [4]byte
					if _, err := io.ReadFull(client, ip[:]); err != nil {
						return
					}
					host = net.IP(ip[:]).String()
				case 3:
					var size [1]byte
					if _, err := io.ReadFull(client, size[:]); err != nil {
						return
					}
					name := make([]byte, int(size[0]))
					if _, err := io.ReadFull(client, name); err != nil {
						return
					}
					host = string(name)
				default:
					unexpected.Add(1)
					return
				}
				var port [2]byte
				if _, err := io.ReadFull(client, port[:]); err != nil {
					return
				}
				if host != "127.0.0.1" || port != [2]byte{0, 1} {
					unexpected.Add(1)
					return
				}
				connects.Add(1)
				upstream, err := net.DialTimeout("tcp", destination, 2*time.Second)
				if err != nil {
					return
				}
				defer upstream.Close()
				client.Write([]byte{5, 0, 0, 1, 127, 0, 0, 1, 0, 1})
				go io.Copy(upstream, client)
				io.Copy(client, upstream)
			}()
		}
	}()
	return "socks5://" + listener.Addr().String(), &connects, &unexpected
}

func TestSOCKS5ALPNNeverRacesH3OrReplaysPost(t *testing.T) {
	for _, mode := range []string{"auto", "h1", "h2", "response-error"} {
		t.Run(mode, func(t *testing.T) {
			reply := []byte("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
			if mode == "response-error" {
				reply = nil
			}
			fixture := newALPNH1Fixture(t, []string{"http/1.1"}, reply)
			proxy, connects, unexpected := socksALPNProxy(t, strings.TrimPrefix(strings.TrimSuffix(fixture.url, "/post"), "https://"))
			protocol := mode
			if mode == "response-error" {
				protocol = "auto"
			}
			result, err := executeWithRoots(Input{Preset: "chrome-152-windows", Protocol: protocol, Proxy: &Proxy{URL: proxy},
				Request: Draft{Method: "POST", URL: "https://127.0.0.1:1/post", Body: "b25jZQ==", Headers: []Header{{"Cookie", "a=1"}, {"X-Middle", "middle"}, {"cookie", "b=2"}}}}, fixture.roots)
			if mode == "h2" || mode == "response-error" {
				if err == nil {
					t.Fatal("strict H2/failed response succeeded")
				}
			} else if err != nil || result["response"].(map[string]any)["version"] != "HTTP/1.1" {
				t.Fatal("SOCKS5 H1 failed", err)
			}
			awaitClosed(t, fixture)
			wantRequests := int32(1)
			if mode == "h2" {
				wantRequests = 0
			}
			if connects.Load() != 1 || unexpected.Load() != 0 || fixture.hellos.Load() != 1 || fixture.requests.Load() != wantRequests {
				t.Fatal("SOCKS5 used UDP/extra tunnel/handshake/POST", connects.Load(), unexpected.Load(), fixture.hellos.Load(), fixture.requests.Load())
			}
		})
	}
}

func TestALPNH1FramingHasOneContentLengthAtItsOriginalPosition(t *testing.T) {
	for _, name := range []string{"content-length", "Content-length", "Content-Length"} {
		t.Run(name, func(t *testing.T) {
			fixture := newALPNH1Fixture(t, []string{"http/1.1"}, []byte("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"))
			input := Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: fixture.url, Body: "b25jZQ==",
				Headers: []Header{{"X-Before", "first"}, {"host", "captured.invalid"}, {name, "4"}, {"Cookie", "a=1"}, {"X-Between", "middle"}, {"cookie", "b=2"}, {"X-After", "last"}}}}
			before := append([]Header(nil), input.Request.Headers...)
			if _, err := executeWithRoots(input, fixture.roots); err != nil {
				t.Fatal(err)
			}
			actual := <-fixture.seen
			count, hosts := 0, 0
			ordered := []string{}
			for _, line := range actual.head {
				key, _, ok := strings.Cut(line, ":")
				if ok && strings.EqualFold(key, "host") {
					hosts++
					if line != "Host: "+strings.TrimSuffix(strings.TrimPrefix(fixture.url, "https://"), "/post") || actual.head[1] != line {
						t.Fatal("Host was duplicated, not first, or kept stale URL", actual.head)
					}
				}
				if ok && strings.EqualFold(key, "content-length") {
					count++
					ordered = append(ordered, line)
				}
				if strings.HasPrefix(line, "X-") || strings.HasPrefix(line, "Cookie:") {
					ordered = append(ordered, line)
				}
			}
			want := []string{"X-Before: first", "Content-Length: 4", "Cookie: a=1; b=2", "X-Between: middle", "X-After: last"}
			if count != 1 || hosts != 1 || string(actual.body) != "once" || !reflect.DeepEqual(ordered, want) || !reflect.DeepEqual(input.Request.Headers, before) {
				t.Fatal("H1 framing/order/input changed", count, ordered, string(actual.body))
			}
			awaitClosed(t, fixture)
		})
	}
}

func TestNegotiatedHTTP2PostUsesOneTLSConnection(t *testing.T) {
	for _, route := range []string{"direct", "socks5"} {
		t.Run(route, func(t *testing.T) {
			var posts, hellos atomic.Int32
			server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				body, err := io.ReadAll(r.Body)
				if err != nil || r.Method != "POST" || r.ProtoMajor != 2 || string(body) != "once" {
					t.Error("actual H2 POST changed", r.Method, r.Proto, string(body), err)
				}
				posts.Add(1)
				w.Write(body)
			}))
			server.EnableHTTP2 = true
			server.TLS = &tls.Config{GetConfigForClient: func(*tls.ClientHelloInfo) (*tls.Config, error) { hellos.Add(1); return nil, nil }}
			server.StartTLS()
			defer server.Close()
			roots := x509.NewCertPool()
			roots.AddCert(server.Certificate())
			input := Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: server.URL, Body: "b25jZQ==", Headers: []Header{{"user-agent", "alpn-test"}, {"cookie", "a=1"}, {"x-between", "middle"}, {"cookie", "b=2"}}}}
			var connects, unexpected *atomic.Int32
			if route == "socks5" {
				proxy, c, u := socksALPNProxy(t, strings.TrimPrefix(server.URL, "https://"))
				connects, unexpected = c, u
				input.Proxy = &Proxy{URL: proxy}
				input.Request.URL = "https://127.0.0.1:1/post"
			}
			result, err := executeWithRoots(input, roots)
			if err != nil {
				t.Fatal(err)
			}
			response := result["response"].(map[string]any)
			if response["version"] != "HTTP/2" || response["bodyBase64"] != "b25jZQ==" || response["upstreamTls"].(map[string]any)["alpn"] != "h2" {
				t.Fatal("H2 response/TLS changed")
			}
			if posts.Load() != 1 || hellos.Load() != 1 {
				t.Fatal("H2 POST/handshake repeated", posts.Load(), hellos.Load())
			}
			if connects != nil && (connects.Load() != 1 || unexpected.Load() != 0) {
				t.Fatal("SOCKS H2 raced H3 or redialed")
			}
		})
	}
}
