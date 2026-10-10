package main

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"github.com/sardanioss/httpcloak/fingerprint"
	tr "github.com/sardanioss/httpcloak/transport"
	utls "github.com/sardanioss/utls"
	"strings"
)

// One helper process serves one request: custom profiles cannot leak across requests.
func applyClientHello(in *Input) (tr.Protocol, string, error) {
	if in.Request.TLS.ClientHelloHex == nil {
		return tr.ProtocolHTTP2, "", nil
	}
	fail := func(s string) (tr.Protocol, string, error) {
		return tr.ProtocolHTTP2, "", fmt.Errorf("ClientHello Hex: %s", s)
	}
	if !strings.HasPrefix(in.Request.URL, "https://") {
		return fail("requires an HTTPS URL")
	}
	value := *in.Request.TLS.ClientHelloHex
	if len(value) > 400000 {
		return fail("input exceeds size limit")
	}
	value = strings.Join(strings.Fields(value), "")
	raw, err := hex.DecodeString(value)
	if err != nil {
		return fail("invalid hexadecimal; paste the complete TLS record without offsets")
	}
	if len(raw) < 9 || len(raw) > 65540 || raw[0] != 22 || raw[5] != 1 {
		return fail("expected one complete TLS ClientHello record (16 ... 01 ...)")
	}
	if int(raw[3])<<8|int(raw[4]) != len(raw)-5 {
		return fail("TLS record length mismatch; fragmented/multiple records are not supported")
	}
	spec, err := fingerprint.SpecFromRawClientHello(raw, false, false)
	if err != nil {
		return fail(err.Error())
	}
	protocol := tr.ProtocolHTTP1
	for _, ext := range spec.Extensions {
		switch e := ext.(type) {
		case *utls.ALPNExtension:
			for _, p := range e.AlpnProtocols {
				if p == "h2" {
					protocol = tr.ProtocolHTTP2
				} else if p != "http/1.1" {
					return fail("only h2 and http/1.1 ALPN are supported")
				}
			}
		case *utls.FakePreSharedKeyExtension, *utls.UtlsPreSharedKeyExtension:
			return fail("captured PSK/session resumption cannot be reused; capture a fresh handshake")
		}
	}
	// Inspect raw extension IDs as ECH implementations have multiple Go types.
	pos := 43
	if pos >= len(raw) {
		return fail("truncated ClientHello")
	}
	pos += 1 + int(raw[pos])
	if pos+2 > len(raw) {
		return fail("truncated cipher suites")
	}
	n := int(raw[pos])<<8 | int(raw[pos+1])
	pos += 2 + n
	if pos >= len(raw) {
		return fail("truncated compression methods")
	}
	pos += 1 + int(raw[pos])
	if pos < len(raw) {
		if pos+2 > len(raw) {
			return fail("truncated extensions")
		}
		end := pos + 2 + (int(raw[pos])<<8 | int(raw[pos+1]))
		pos += 2
		if end != len(raw) {
			return fail("extension length mismatch")
		}
		for pos < end {
			if pos+4 > end {
				return fail("truncated extension")
			}
			id := int(raw[pos])<<8 | int(raw[pos+1])
			n := int(raw[pos+2])<<8 | int(raw[pos+3])
			pos += 4 + n
			if pos > end {
				return fail("truncated extension data")
			}
			if id == 65037 || id == 41 || id == 42 || id == 57 {
				return fail(fmt.Sprintf("extension %d (ECH/PSK/early data/QUIC) is not supported by this import mode", id))
			}
		}
	}
	preset := fingerprint.GetStrict(in.Preset)
	if preset == nil {
		return fail("unknown HTTP profile")
	}
	preset.RawClientHello = raw
	preset.RawPSKClientHello = nil
	preset.RawBluntMimicry = false
	preset.RawPermuteExtensions = false
	base := in.Preset
	in.Preset = "capture-custom-hello"
	fingerprint.Register(in.Preset, preset)
	return protocol, fmt.Sprintf("TLS: imported ClientHello SHA256=%x; HTTP settings: %s", sha256.Sum256(raw), base) + "; fresh keys/SNI generated, not byte-for-byte replay", nil
}
