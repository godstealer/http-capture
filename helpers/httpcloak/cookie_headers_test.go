package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/hex"
	"fmt"
	"io"
	"net"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
	"github.com/sardanioss/net/http2"
	"github.com/sardanioss/net/http2/hpack"
)

func assertOnlyCookiePolicyChanged(t *testing.T, baseName, exactName string) {
	t.Helper()
	want := fingerprint.GetStrict(baseName)
	got := fingerprint.GetStrict(exactName)
	if want == nil || got == nil || got.H2Config == nil || !got.H2DisableCookieSplit() {
		t.Fatal("missing preset or exact Cookie policy")
	}
	want.Name = exactName
	want.BasedOn = baseName
	disableSplit := true
	want.H2Config.DisableCookieSplit = &disableSplit
	if !reflect.DeepEqual(got, want) {
		t.Fatal("derived preset changed TLS, SETTINGS, pseudo order or another unrelated profile field")
	}
}

func TestExactCookiePresetPreservesBrowserProfile(t *testing.T) {
	for _, base := range []string{"chrome-152-windows", "firefox-148-windows"} {
		t.Run(base, func(t *testing.T) {
			before := fingerprint.GetStrict(base)
			if before == nil || before.H2DisableCookieSplit() {
				t.Fatal("fixture must normally split cookies")
			}
			name, err := exactCookiePreset(base)
			if err != nil {
				t.Fatal(err)
			}
			defer fingerprint.Unregister(name)
			assertOnlyCookiePolicyChanged(t, base, name)
			if name == base || !reflect.DeepEqual(before, fingerprint.GetStrict(base)) {
				t.Fatal("the browser's global base profile was modified")
			}
			// The derived registry entry is a clone too: subsequent mutation of
			// a returned profile must affect neither it nor the browser profile.
			copy := fingerprint.GetStrict(name)
			copy.H2Config.PseudoHeaderOrder[0] = ":changed"
			assertOnlyCookiePolicyChanged(t, base, name)
		})
	}
	if _, err := exactCookiePreset("does-not-exist"); err == nil {
		t.Fatal("unknown base profile silently fell back")
	}
}

func TestExactCookiePresetConcurrentImportedHellos(t *testing.T) {
	raw := capturedHello(t, []string{"h2", "http/1.1"})
	const count = 16
	type result struct {
		hello, exact string
		err          error
	}
	results := make(chan result, count)
	var wg sync.WaitGroup
	for index := 0; index < count; index++ {
		wg.Add(1)
		go func(index int) {
			defer wg.Done()
			// Vary a ClientHello random byte without changing its framing.
			customRaw := bytes.Clone(raw)
			customRaw[20] = byte(index)
			value := hex.EncodeToString(customRaw)
			in := Input{Preset: "chrome-152-windows"}
			in.Request.URL = "https://example.test/"
			in.Request.TLS.ClientHelloHex = &value
			protocol, _, err := applyClientHello(&in)
			if err != nil || protocol != tr.ProtocolHTTP2 {
				results <- result{err: fmt.Errorf("import hello: protocol=%v error=%v", protocol, err)}
				return
			}
			defer fingerprint.Unregister(in.Preset)
			name, err := exactCookiePreset(in.Preset)
			if err != nil {
				results <- result{err: err}
				return
			}
			defer fingerprint.Unregister(name)
			base, exact := fingerprint.GetStrict(in.Preset), fingerprint.GetStrict(name)
			if base == nil || exact == nil || !bytes.Equal(base.RawClientHello, customRaw) || !bytes.Equal(exact.RawClientHello, customRaw) {
				results <- result{err: fmt.Errorf("concurrent request replaced imported hello %d", index)}
				return
			}
			// Normalize the two deliberate metadata differences and policy.
			exact.Name, exact.BasedOn = base.Name, base.BasedOn
			exact.H2Config.DisableCookieSplit = base.H2Config.DisableCookieSplit
			if !reflect.DeepEqual(base, exact) {
				results <- result{err: fmt.Errorf("imported TLS or other H2 fields changed")}
				return
			}
			results <- result{hello: in.Preset, exact: name}
		}(index)
	}
	wg.Wait()
	close(results)
	seen := map[string]bool{}
	for item := range results {
		if item.err != nil {
			t.Fatal(item.err)
		}
		for _, name := range []string{item.hello, item.exact} {
			if seen[name] || fingerprint.GetStrict(name) != nil {
				t.Fatal("internal name reused or not removed", name)
			}
			seen[name] = true
		}
	}
	if len(seen) != 2*count {
		t.Fatal("missing concurrent profiles")
	}
	if fingerprint.GetStrict("chrome-152-windows").RawClientHello != nil {
		t.Fatal("imported hello leaked to the shipped browser profile")
	}
}

func TestJoinCookieFieldsAndHTTP1Wire(t *testing.T) {
	input := []fingerprint.HeaderPair{
		{Key: "X-Start", Value: "one"}, {Key: "cOoKiE", Value: "same=1; encoded=%3B%3D"},
		{Key: "Accept", Value: "*/*"}, {Key: "cookie", Value: "same=2"},
		{Key: "Set-Cookie", Value: "response=3; Path=/"}, {Key: "COOKIE", Value: "empty="},
		{Key: "X-End", Value: "two"},
	}
	before := append([]fingerprint.HeaderPair(nil), input...)
	want := []fingerprint.HeaderPair{input[0], {Key: "cOoKiE", Value: "same=1; encoded=%3B%3D; same=2; empty="}, input[2], input[4], input[6]}
	joined, changed := joinCookieFields(input)
	if !changed || !reflect.DeepEqual(joined, want) || !reflect.DeepEqual(input, before) {
		t.Fatal("H1 merge changed values, field order, spelling or the input", joined)
	}
	for _, headers := range [][]fingerprint.HeaderPair{nil, {}, {{Key: "Cookie", Value: "a=1"}}, {{Key: "Set-Cookie", Value: "a=1"}, {Key: "Set-Cookie", Value: "b=2"}}} {
		got, changed := joinCookieFields(headers)
		if changed || !reflect.DeepEqual(got, headers) {
			t.Fatal("unrelated or single fields changed")
		}
	}

	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	type wireResult struct {
		lines []string
		err   error
	}
	received := make(chan wireResult, 1)
	go func() {
		connection, err := listener.Accept()
		if err != nil {
			received <- wireResult{err: err}
			return
		}
		defer connection.Close()
		connection.SetDeadline(time.Now().Add(5 * time.Second))
		reader := bufio.NewReader(connection)
		lines := []string{}
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				received <- wireResult{err: err}
				return
			}
			if line == "\r\n" {
				break
			}
			line = strings.TrimSuffix(strings.TrimSuffix(line, "\n"), "\r")
			if !strings.HasPrefix(line, "GET ") && !strings.HasPrefix(strings.ToLower(line), "host:") {
				lines = append(lines, line)
			}
		}
		_, err = fmt.Fprint(connection, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
		received <- wireResult{lines: lines, err: err}
	}()
	in := Input{Preset: "chrome-152-windows", Request: Draft{Method: "GET", URL: "http://" + listener.Addr().String() + "/"}}
	for _, header := range input {
		in.Request.Headers = append(in.Request.Headers, Header{Name: header.Key, Value: header.Value})
	}
	out, err := execute(in)
	if err != nil {
		t.Fatal(err)
	}
	actual := <-received
	wantLines := []string{}
	for _, header := range want {
		wantLines = append(wantLines, header.Key+": "+header.Value)
	}
	if actual.err != nil || !reflect.DeepEqual(actual.lines, wantLines) {
		t.Fatalf("actual H1 wire fields = %v, error=%v", actual.lines, actual.err)
	}
	if out["response"].(map[string]any)["sentRequestHeaders"] != nil {
		t.Fatal("helper fabricated sent header evidence")
	}
	if !strings.Contains(strings.Join(out["notes"].([]string), "\n"), "joins repeated Cookie") {
		t.Fatal("missing H1 Cookie forwarding explanation")
	}
}

type h2CookieWireResult struct {
	fields []hpack.HeaderField
	err    error
}

// Decode raw HPACK fields rather than use an HTTP handler: a server's request
// parser may already have joined cookies, hiding the actual wire boundaries.
func h2CookieWireServer(t *testing.T) (string, *x509.CertPool, <-chan h2CookieWireResult) {
	t.Helper()
	fixture := httptest.NewTLSServer(nil)
	certificate := fixture.TLS.Certificates[0]
	roots := x509.NewCertPool()
	roots.AddCert(fixture.Certificate())
	fixture.Close()
	listener, err := tls.Listen("tcp", "127.0.0.1:0", &tls.Config{Certificates: []tls.Certificate{certificate}, NextProtos: []string{"h2"}})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { listener.Close() })
	received := make(chan h2CookieWireResult, 1)
	go func() {
		resultSent := false
		fields, err := func() ([]hpack.HeaderField, error) {
			connection, err := listener.Accept()
			if err != nil {
				return nil, err
			}
			defer connection.Close()
			connection.SetDeadline(time.Now().Add(8 * time.Second))
			preface := make([]byte, len(http2.ClientPreface))
			if _, err := io.ReadFull(connection, preface); err != nil {
				return nil, err
			}
			if string(preface) != http2.ClientPreface {
				return nil, fmt.Errorf("not an H2 client preface")
			}
			framer := http2.NewFramer(connection, connection)
			framer.ReadMetaHeaders = hpack.NewDecoder(4096, nil)
			if err := framer.WriteSettings(); err != nil {
				return nil, err
			}
			for {
				frame, err := framer.ReadFrame()
				if err != nil {
					return nil, err
				}
				switch frame := frame.(type) {
				case *http2.SettingsFrame:
					if !frame.IsAck() {
						if err := framer.WriteSettingsAck(); err != nil {
							return nil, err
						}
					}
				case *http2.PingFrame:
					if !frame.Flags.Has(http2.FlagPingAck) {
						if err := framer.WritePing(true, frame.Data); err != nil {
							return nil, err
						}
					}
				case *http2.MetaHeadersFrame:
					fields := append([]hpack.HeaderField(nil), frame.Fields...)
					var block bytes.Buffer
					encoder := hpack.NewEncoder(&block)
					if err := encoder.WriteField(hpack.HeaderField{Name: ":status", Value: "200"}); err != nil {
						return nil, err
					}
					if err := framer.WriteHeaders(http2.HeadersFrameParam{StreamID: frame.StreamID, BlockFragment: block.Bytes(), EndHeaders: true, EndStream: true}); err != nil {
						return fields, err
					}
					received <- h2CookieWireResult{fields: fields}
					resultSent = true
					// Keep the fixture alive until the client closes its transport.
					// Closing with unread SETTINGS/WINDOW_UPDATE frames can reset
					// the TCP connection on Windows and discard our response.
					for {
						if _, err := framer.ReadFrame(); err != nil {
							return nil, nil
						}
					}
				}
			}
		}()
		if !resultSent {
			received <- h2CookieWireResult{fields: fields, err: err}
		}
	}()
	return "https://" + listener.Addr().String() + "/cookie-test", roots, received
}

func TestExactCookieHTTP2WireBoundaries(t *testing.T) {
	for _, base := range []string{"chrome-152-windows", "firefox-148-windows"} {
		for _, cookies := range [][]string{{"same=1; same=2; encoded=%3B%3D; empty="}, {"same=1; quoted=\"x;y\"", "same=2; empty="}} {
			t.Run(fmt.Sprintf("%s/%d-fields", base, len(cookies)), func(t *testing.T) {
				url, roots, received := h2CookieWireServer(t)
				name, err := exactCookiePreset(base)
				if err != nil {
					t.Fatal(err)
				}
				defer fingerprint.Unregister(name)
				transport := tr.NewTransportWithConfig(name, nil, &tr.TransportConfig{TLSOnly: true})
				defer transport.Close()
				transport.SetProtocol(tr.ProtocolHTTP2)
				// Trust only this test's certificate; production execute() keeps
				// the system roots and never enables InsecureSkipVerify.
				transport.SetTLSVerify(&tr.TLSVerify{RootCAs: roots})
				pairs := []fingerprint.HeaderPair{{Key: "x-start", Value: "one"}, {Key: "user-agent", Value: "capture-cookie-test"}}
				want := []string{"x-start:one", "user-agent:capture-cookie-test"}
				for index, value := range cookies {
					pairs = append(pairs, fingerprint.HeaderPair{Key: "cookie", Value: value}, fingerprint.HeaderPair{Key: "x-between", Value: fmt.Sprint(index)})
					want = append(want, "cookie:"+value, "x-between:"+fmt.Sprint(index))
				}
				response, err := transport.DoStream(context.Background(), &tr.Request{Method: "GET", URL: url, ExactHeaders: pairs, Timeout: 8 * time.Second})
				if err != nil {
					t.Fatal(err)
				}
				response.Close()
				actual := <-received
				if actual.err != nil {
					t.Fatal(actual.err)
				}
				got := []string{}
				pseudo := []string{}
				for _, field := range actual.fields {
					if strings.HasPrefix(field.Name, ":") {
						pseudo = append(pseudo, field.Name)
					} else {
						got = append(got, field.Name+":"+field.Value)
					}
				}
				if !reflect.DeepEqual(got, want) {
					t.Fatalf("actual HPACK field boundaries/order: got %v, want %v", got, want)
				}
				if !reflect.DeepEqual(pseudo, fingerprint.GetStrict(base).H2PseudoHeaderOrder()) {
					t.Fatal("preset pseudo-header order changed", pseudo)
				}
			})
		}
	}
}
