package main

import (
	"fmt"
	"strings"
	"sync"

	"github.com/sardanioss/httpcloak/fingerprint"
)

// All helper-owned profiles use separate names. In addition to keeping the
// shipped browser profiles intact, this avoids one request's imported hello or
// cookie policy being replaced by another request while a transport is built.
var internalProfiles struct {
	sync.Mutex
	sequence uint64
}

func registerInternalPreset(kind string, preset *fingerprint.Preset) (string, error) {
	if preset == nil {
		return "", fmt.Errorf("cannot register an empty internal preset")
	}
	internalProfiles.Lock()
	defer internalProfiles.Unlock()
	for attempt := 0; attempt < 64; attempt++ {
		internalProfiles.sequence++
		name := fmt.Sprintf("http-capture-%s-%d", kind, internalProfiles.sequence)
		preset.Name = name
		if err := fingerprint.RegisterStrict(name, preset); err == nil {
			return name, nil
		}
	}
	return "", fmt.Errorf("cannot allocate a unique internal preset")
}

// ExactHeaders controls the field boundaries, including Cookie. Browser
// presets ordinarily crumble cookies again in the H2 encoder; change only
// that policy on a deep-cloned preset, retaining all TLS and other H2 fields.
// Call this after applying an imported ClientHello and unregister after use.
func exactCookiePreset(base string) (string, error) {
	disableSplit := true
	preset, err := fingerprint.BuildPreset(&fingerprint.PresetSpec{
		BasedOn: base,
		HTTP2:   &fingerprint.HTTP2Spec{DisableCookieSplit: &disableSplit},
	})
	if err != nil {
		return "", fmt.Errorf("prepare exact Cookie fields: %w", err)
	}
	return registerInternalPreset("cookie-fields", preset)
}

// RFC 9113 section 8.2.3 requires split Cookie fields to be joined with "; "
// when forwarded to HTTP/1.1. Keep the first field's name and position, and do
// not reorder or change unrelated duplicate fields (especially Set-Cookie).
func joinCookieFields(headers []fingerprint.HeaderPair) ([]fingerprint.HeaderPair, bool) {
	values := []string{}
	for _, header := range headers {
		if strings.EqualFold(header.Key, "cookie") {
			values = append(values, header.Value)
		}
	}
	if len(values) < 2 {
		return headers, false
	}
	result := make([]fingerprint.HeaderPair, 0, len(headers)-len(values)+1)
	joined := false
	for _, header := range headers {
		if strings.EqualFold(header.Key, "cookie") {
			if joined {
				continue
			}
			header.Value = strings.Join(values, "; ")
			joined = true
		}
		result = append(result, header)
	}
	return result, true
}
