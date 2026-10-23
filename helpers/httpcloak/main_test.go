package main

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

func TestCertificateRootsExtendTrustAndRejectIncompleteIdentity(t *testing.T) {
	s := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("trusted")) }))
	defer s.Close()
	in := Input{Preset: "chrome-152-windows", Request: Draft{Method: "GET", URL: s.URL}}
	if _, err := execute(in); err == nil {
		t.Fatal("untrusted server accepted")
	}
	in.Security = &Security{TrustedCAPEM: []string{string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: s.Certificate().Raw}))}}
	out, err := execute(in)
	if err != nil {
		t.Fatal(err)
	}
	if out["response"].(map[string]any)["status"] != 200 {
		t.Fatal(out)
	}
	in.Security.TrustedCAPEM = []string{"invalid"}
	if _, err = execute(in); err == nil {
		t.Fatal("invalid CA accepted")
	}
	identity := "private identity must never be printed"
	in.Security = &Security{ClientPrivateKeyPEM: &identity}
	if _, err = execute(in); err == nil || !strings.Contains(err.Error(), "must be provided together") || strings.Contains(err.Error(), identity) {
		t.Fatal("incomplete identity must fail without leaking material")
	}
}

func TestClientCertificateHandshakeH1AndH2(t *testing.T) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	template := &x509.Certificate{SerialNumber: big.NewInt(42), NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour), KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth}}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	keyDER, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	certPEM := string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))
	keyPEM := string(pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: keyDER}))
	for _, protocol := range []string{"h1", "h2"} {
		t.Run(protocol, func(t *testing.T) {
			server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.TLS == nil || len(r.TLS.PeerCertificates) != 1 || !bytes.Equal(r.TLS.PeerCertificates[0].Raw, der) {
					t.Error("client identity missing or changed")
				}
				w.Write([]byte("mutual tls"))
			}))
			server.EnableHTTP2 = protocol == "h2"
			server.TLS = &tls.Config{ClientAuth: tls.RequireAnyClientCert, VerifyPeerCertificate: func(raw [][]byte, _ [][]*x509.Certificate) error {
				if len(raw) != 1 || !bytes.Equal(raw[0], der) {
					return fmt.Errorf("unexpected client certificate")
				}
				return nil
			}}
			server.StartTLS()
			defer server.Close()
			roots := x509.NewCertPool()
			roots.AddCert(server.Certificate())
			input := Input{Preset: "chrome-155-windows", Request: Draft{Method: "GET", URL: server.URL}, Security: &Security{ClientCertificatePEM: &certPEM, ClientPrivateKeyPEM: &keyPEM}}
			result, err := executeWithRoots(input, roots)
			if err != nil {
				t.Fatal(err)
			}
			response := result["response"].(map[string]any)
			wantVersion := "HTTP/1.1"
			if protocol == "h2" {
				wantVersion = "HTTP/2"
			}
			if response["status"] != 200 || response["version"] != wantVersion {
				t.Fatalf("unexpected response: %v", response)
			}
			input.Security = nil
			if _, err := executeWithRoots(input, roots); err == nil {
				t.Fatal("server accepted request without client certificate")
			}
		})
	}
}

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

func TestLargeBinaryRoundTrip(t *testing.T) {
	payload := bytes.Repeat([]byte{0, 255, 128, 17}, 4*1024*1024)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil || !bytes.Equal(body, payload) {
			t.Error("large upload mismatch")
			w.WriteHeader(400)
			return
		}
		w.Header().Set("Content-Type", "application/octet-stream")
		w.Write(body)
	}))
	defer server.Close()
	encoded := base64.StdEncoding.EncodeToString(payload)
	input := Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: server.URL, Body: encoded}}
	out, err := executeOwned(&input)
	if err != nil {
		t.Fatal(err)
	}
	if input.Request.Body != "" {
		t.Fatal("encoded upload retained after decoding")
	}
	if out["response"].(map[string]any)["bodyBase64"] != encoded {
		t.Fatal("large response mismatch")
	}
}

type boundedWrite struct {
	bytes.Buffer
	largest int
}

func (w *boundedWrite) Write(part []byte) (int, error) {
	if len(part) > w.largest {
		w.largest = len(part)
	}
	if len(part) > 64*1024 {
		return 0, fmt.Errorf("unbounded response write: %d bytes", len(part))
	}
	return w.Buffer.Write(part)
}

func TestResponseJSONStreamsBase64WithoutOneLargeWrite(t *testing.T) {
	body := bytes.Repeat([]byte{0, 255, 128, 17}, 512*1024)
	result := &helperResult{metadata: map[string]any{
		"notes":    []string{"response \"quoted\" 中文"},
		"response": map[string]any{"status": 200, "version": "HTTP/2", "headers": []Header{{"X-Test", "yes"}}, "rawHeadBase64": nil},
	}, response: &negotiatedResponse{ReadCloser: io.NopCloser(bytes.NewReader(body))}}
	var output boundedWrite
	if err := writeResponse(&output, result); err != nil {
		t.Fatal(err)
	}
	var parsed struct {
		Notes    []string `json:"notes"`
		Response struct {
			Body string `json:"bodyBase64"`
		} `json:"response"`
	}
	if err := json.Unmarshal(output.Bytes(), &parsed); err != nil {
		t.Fatal(err)
	}
	if parsed.Notes[0] != "response \"quoted\" 中文" || parsed.Response.Body != base64.StdEncoding.EncodeToString(body) {
		t.Fatal("streamed response changed JSON bytes")
	}
	if output.largest > 64*1024 {
		t.Fatalf("largest write: %d", output.largest)
	}
	var empty bytes.Buffer
	if err := writeResponse(&empty, &helperResult{metadata: map[string]any{"notes": []string{}, "response": map[string]any{}}, response: &negotiatedResponse{ReadCloser: io.NopCloser(bytes.NewReader(nil))}}); err != nil {
		t.Fatal(err)
	}
	if !json.Valid(empty.Bytes()) {
		t.Fatal("empty metadata result is invalid JSON")
	}
}

type responseReadError struct{}

func (responseReadError) Read([]byte) (int, error) {
	return 0, fmt.Errorf("response stream interrupted")
}

func TestStreamReadFailureCannotBecomeSuccessfulResponse(t *testing.T) {
	result := &helperResult{
		response: &negotiatedResponse{ReadCloser: io.NopCloser(io.MultiReader(bytes.NewReader([]byte("partial")), responseReadError{})), StatusCode: 200, Protocol: "h2"},
		preset:   "chrome-152-windows",
	}
	var output bytes.Buffer
	if err := writeResponse(&output, result); err != nil {
		t.Fatal(err)
	}
	var reply struct {
		Response struct {
			Body string `json:"bodyBase64"`
		} `json:"response"`
		Error string `json:"error"`
	}
	if err := json.Unmarshal(output.Bytes(), &reply); err != nil {
		t.Fatal(err)
	}
	if reply.Error != "response stream interrupted" || reply.Response.Body != base64.StdEncoding.EncodeToString([]byte("partial")) {
		t.Fatalf("partial response must include an error: %+v", reply)
	}
}

type responseProgressWriter struct {
	bytes.Buffer
	progress chan struct{}
}

func (w *responseProgressWriter) Write(part []byte) (int, error) {
	if len(part) > 100 {
		select {
		case w.progress <- struct{}{}:
		default:
		}
	}
	return w.Buffer.Write(part)
}

func TestResponseBodyIsWrittenBeforeNetworkEOF(t *testing.T) {
	reader, writer := io.Pipe()
	result := &helperResult{response: &negotiatedResponse{ReadCloser: reader, StatusCode: 200, Protocol: "h2"}, preset: "chrome-152-windows"}
	output := &responseProgressWriter{progress: make(chan struct{}, 1)}
	completed := make(chan error, 1)
	go func() { completed <- writeResponse(output, result) }()
	first := bytes.Repeat([]byte("abc"), 16*1024)
	if _, err := writer.Write(first); err != nil {
		t.Fatal(err)
	}
	select {
	case <-output.progress:
	case <-time.After(3 * time.Second):
		writer.Close()
		<-completed
		t.Fatal("response body was buffered until network EOF")
	}
	if err := writer.Close(); err != nil {
		t.Fatal(err)
	}
	if err := <-completed; err != nil {
		t.Fatal(err)
	}
	var reply struct {
		Response struct {
			Body string `json:"bodyBase64"`
		} `json:"response"`
		Error string `json:"error"`
	}
	if err := json.Unmarshal(output.Bytes(), &reply); err != nil {
		t.Fatal(err)
	}
	if reply.Error != "" || reply.Response.Body != base64.StdEncoding.EncodeToString(first) {
		t.Fatal("streamed body changed")
	}
}

func TestMainCLIStreamsResponse(t *testing.T) {
	if os.Getenv("HTTP_CAPTURE_HELPER_TEST_PROCESS") == "1" {
		main()
		os.Exit(0)
	}
	responseBody := bytes.Repeat([]byte{0, 255, 128, 17}, 32*1024)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			t.Errorf("method = %s", r.Method)
		}
		requestBody, err := io.ReadAll(r.Body)
		if err != nil || string(requestBody) != "request bytes" {
			t.Errorf("request body = %q, %v", requestBody, err)
		}
		w.Header().Set("Content-Type", "application/octet-stream")
		_, _ = w.Write(responseBody)
	}))
	defer server.Close()
	input, err := json.Marshal(Input{Preset: "chrome-152-windows", Request: Draft{Method: "POST", URL: server.URL, Body: base64.StdEncoding.EncodeToString([]byte("request bytes"))}})
	if err != nil {
		t.Fatal(err)
	}
	command := exec.Command(os.Args[0], "-test.run=^TestMainCLIStreamsResponse$")
	command.Env = append(os.Environ(), "HTTP_CAPTURE_HELPER_TEST_PROCESS=1")
	command.Stdin = bytes.NewReader(input)
	output, err := command.Output()
	if err != nil {
		t.Fatal(err)
	}
	var reply struct {
		Response struct {
			Body string `json:"bodyBase64"`
		} `json:"response"`
		Error string `json:"error"`
	}
	if err := json.Unmarshal(output, &reply); err != nil {
		t.Fatal(err)
	}
	if reply.Error != "" || reply.Response.Body != base64.StdEncoding.EncodeToString(responseBody) {
		t.Fatalf("CLI response changed: %s", reply.Error)
	}
}
